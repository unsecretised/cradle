//! A minimal MCP server (streamable HTTP, JSON responses) that exposes Cradle's tools to
//! the Claude Code and Codex CLIs. See `.agents/roadmap/FEATURE_CLI_BACKENDS.md`.
//!
//! The server does not run tools itself. Each `tools/call` is handed to the agent loop
//! over a channel, so mode limits, approvals and the path guard all still apply.
//!
//! It listens on 127.0.0.1 only and requires a random bearer token, so other local
//! processes cannot call it. Requests with a foreign `Host` or any `Origin` header are
//! refused (DNS-rebinding protection).

use std::io;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

const MAX_BODY: usize = 16 * 1024 * 1024;
const SERVER_NAME: &str = "cradle";
/// Used when the client does not say which version it speaks.
const DEFAULT_PROTOCOL: &str = "2025-06-18";

/// A tool as advertised over MCP.
#[derive(Debug, Clone)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    /// Advertised as `readOnlyHint`. Tools that are not read-only are advertised as
    /// non-destructive: Cradle approves them itself, so the CLI should not ask again.
    pub read_only: bool,
}

/// A tool call from the CLI, waiting for the agent's answer.
#[derive(Debug)]
pub struct ToolRequest {
    pub name: String,
    pub arguments: Value,
    pub reply: oneshot::Sender<ToolReply>,
}

#[derive(Debug, Clone)]
pub struct ToolReply {
    pub text: String,
    pub is_error: bool,
}

/// A running server. Dropping it stops accepting connections.
pub struct McpServer {
    url: String,
    token: String,
    task: JoinHandle<()>,
}

impl McpServer {
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The bearer token clients must send.
    pub fn token(&self) -> &str {
        &self.token
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Start serving `tools` on a random local port. Calls arrive on `requests`.
pub async fn start(
    tools: Vec<McpTool>,
    requests: mpsc::UnboundedSender<ToolRequest>,
) -> io::Result<McpServer> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let mut secret = [0u8; 32];
    getrandom::fill(&mut secret).map_err(|e| io::Error::other(e.to_string()))?;
    let token = hex::encode(secret);

    let state = std::sync::Arc::new(State {
        tools,
        requests,
        token: token.clone(),
        hosts: [format!("127.0.0.1:{port}"), format!("localhost:{port}")],
    });
    let task = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let state = state.clone();
            tokio::spawn(async move {
                let _ = serve_connection(stream, &state).await;
            });
        }
    });
    Ok(McpServer {
        url: format!("http://127.0.0.1:{port}/mcp"),
        token,
        task,
    })
}

struct State {
    tools: Vec<McpTool>,
    requests: mpsc::UnboundedSender<ToolRequest>,
    token: String,
    hosts: [String; 2],
}

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// Handle requests on one connection until the client closes it (keep-alive).
async fn serve_connection(stream: TcpStream, state: &State) -> io::Result<()> {
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);
    while let Some(request) = read_request(&mut reader).await? {
        let (status, content_type, body) = handle(&request, state).await;
        let mut head = format!(
            "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: keep-alive\r\n",
            body.len()
        );
        if let Some(ct) = content_type {
            head.push_str(&format!("content-type: {ct}\r\n"));
        }
        if status.starts_with("405") {
            head.push_str("allow: POST\r\n");
        }
        head.push_str("\r\n");
        write.write_all(head.as_bytes()).await?;
        write.write_all(&body).await?;
        write.flush().await?;
    }
    Ok(())
}

async fn read_request(
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
) -> io::Result<Option<Request>> {
    let mut line = String::new();
    if reader.read_line(&mut line).await? == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_owned();
    let path = parts.next().unwrap_or("").to_owned();

    let mut headers = Vec::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            return Ok(None);
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
        }
    }
    let length: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    if length > MAX_BODY {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "body too large"));
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).await?;
    Ok(Some(Request {
        method,
        path,
        headers,
        body,
    }))
}

async fn handle(request: &Request, state: &State) -> (&'static str, Option<&'static str>, Vec<u8>) {
    if std::env::var_os("CRADLE_MCP_DEBUG").is_some() {
        // Debug aid for checking CLI compatibility. The token is never logged.
        let headers: Vec<_> = request
            .headers
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str(),
                    if k == "authorization" {
                        "<redacted>"
                    } else {
                        v
                    },
                )
            })
            .collect();
        eprintln!(
            "mcp {} {} headers={headers:?} body={}",
            request.method,
            request.path,
            String::from_utf8_lossy(&request.body)
        );
    }
    let host_ok = request
        .header("host")
        .is_some_and(|h| state.hosts.iter().any(|allowed| allowed == h));
    if !host_ok || request.header("origin").is_some() {
        return ("403 Forbidden", None, Vec::new());
    }
    let expected = format!("Bearer {}", state.token);
    if request.header("authorization") != Some(expected.as_str()) {
        return ("401 Unauthorized", None, Vec::new());
    }
    if request.path.split('?').next() != Some("/mcp") {
        return ("404 Not Found", None, Vec::new());
    }
    match request.method.as_str() {
        "POST" => {}
        "DELETE" => return ("200 OK", None, Vec::new()),
        // No server-initiated stream: the spec allows refusing GET.
        _ => return ("405 Method Not Allowed", None, Vec::new()),
    }

    let Ok(message) = serde_json::from_slice::<Value>(&request.body) else {
        let error = rpc_error(Value::Null, -32700, "parse error");
        return (
            "400 Bad Request",
            Some("application/json"),
            error.to_string().into_bytes(),
        );
    };
    let replies = match message {
        Value::Array(batch) => {
            let mut replies = Vec::new();
            for item in batch {
                if let Some(reply) = dispatch(item, state).await {
                    replies.push(reply);
                }
            }
            (!replies.is_empty()).then_some(Value::Array(replies))
        }
        single => dispatch(single, state).await,
    };
    match replies {
        Some(body) => (
            "200 OK",
            Some("application/json"),
            body.to_string().into_bytes(),
        ),
        // Only notifications or responses: nothing to answer.
        None => ("202 Accepted", None, Vec::new()),
    }
}

/// Answer one JSON-RPC message. Notifications and responses get no reply.
async fn dispatch(message: Value, state: &State) -> Option<Value> {
    let id = message.get("id").cloned()?;
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return None; // a response to something we never sent
    };
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(DEFAULT_PROTOCOL),
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION")},
            "instructions": "Cradle's tools. They are the only way to read or change the \
                             project; every call is checked by the Cradle harness.",
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({
            "tools": state.tools.iter().map(|t| json!({
                "name": t.name,
                "description": t.description,
                "inputSchema": t.input_schema,
                "annotations": {
                    "readOnlyHint": t.read_only,
                    "destructiveHint": false,
                    "openWorldHint": false,
                },
            })).collect::<Vec<_>>()
        })),
        "tools/call" => call_tool(&params, state).await,
        _ => Err((-32601, format!("method not found: {method}"))),
    };
    Some(match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err((code, message)) => rpc_error(id, code, &message),
    })
}

async fn call_tool(params: &Value, state: &State) -> Result<Value, (i64, String)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((-32602, "missing tool name".to_owned()))?;
    if !state.tools.iter().any(|t| t.name == name) {
        return Err((-32602, format!("unknown tool: {name}")));
    }
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let (reply, answer) = oneshot::channel();
    state
        .requests
        .send(ToolRequest {
            name: name.to_owned(),
            arguments,
            reply,
        })
        .map_err(|_| (-32603, "Cradle is not accepting tool calls".to_owned()))?;
    let reply = answer
        .await
        .map_err(|_| (-32603, "the tool call was cancelled".to_owned()))?;
    Ok(json!({
        "content": [{"type": "text", "text": reply.text}],
        "isError": reply.is_error,
    }))
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn server() -> (McpServer, mpsc::UnboundedReceiver<ToolRequest>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let tools = vec![McpTool {
            name: "read_file".into(),
            description: "Read".into(),
            input_schema: json!({"type": "object"}),
            read_only: true,
        }];
        (start(tools, tx).await.unwrap(), rx)
    }

    async fn post(server: &McpServer, token: &str, body: Value) -> reqwest::Response {
        reqwest::Client::new()
            .post(server.url())
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn initialize_list_and_call() {
        let (server, mut calls) = server().await;
        let token = server.token().to_owned();

        let init = post(
            &server,
            &token,
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}),
        )
        .await
        .json::<Value>()
        .await
        .unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");

        let note = post(
            &server,
            &token,
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        )
        .await;
        assert_eq!(note.status(), 202);

        let list = post(
            &server,
            &token,
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        )
        .await
        .json::<Value>()
        .await
        .unwrap();
        assert_eq!(list["result"]["tools"][0]["name"], "read_file");

        let answer = tokio::spawn(async move {
            let request = calls.recv().await.unwrap();
            assert_eq!(request.arguments, json!({"path": "a"}));
            request
                .reply
                .send(ToolReply {
                    text: "contents".into(),
                    is_error: false,
                })
                .unwrap();
        });
        let call = post(
            &server,
            &token,
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"read_file","arguments":{"path":"a"}}}),
        )
        .await
        .json::<Value>()
        .await
        .unwrap();
        answer.await.unwrap();
        assert_eq!(call["result"]["content"][0]["text"], "contents");
        assert_eq!(call["result"]["isError"], false);
    }

    #[tokio::test]
    async fn refuses_without_token_or_with_origin() {
        let (server, _calls) = server().await;
        let body = json!({"jsonrpc":"2.0","id":1,"method":"tools/list"});
        assert_eq!(post(&server, "wrong", body.clone()).await.status(), 401);

        let with_origin = reqwest::Client::new()
            .post(server.url())
            .bearer_auth(server.token())
            .header("origin", "https://evil.example")
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(with_origin.status(), 403);
    }

    #[tokio::test]
    async fn unknown_tools_are_rejected() {
        let (server, _calls) = server().await;
        let reply = post(
            &server,
            server.token(),
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"bash","arguments":{}}}),
        )
        .await
        .json::<Value>()
        .await
        .unwrap();
        assert_eq!(reply["error"]["code"], -32602);
    }
}

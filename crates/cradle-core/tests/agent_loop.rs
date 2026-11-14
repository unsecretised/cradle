//! End-to-end tests of the agent loop against a scripted fake Anthropic server.
//! They check the harness enforces the rules, whatever the model asks for.
#![cfg(unix)]

use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use cradle_backends::Backend;
use cradle_config::SourceKind;
use cradle_core::{
    AgentCommand, AgentEvent, AgentHandle, AgentSetup, Approval, ClashChoice, ModeSet, ReviewPolicy,
};
use cradle_provider_spec::{
    Auth, Backend as SpecBackend, Capabilities, HttpApi, HttpProvider, ProviderSpec,
};
use cradle_tools::PathGuard;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Serves one scripted SSE response per request and records request bodies.
async fn fake_server(responses: Vec<String>) -> (String, Arc<Mutex<Vec<Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    tokio::spawn(async move {
        for body in responses {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            seen.lock().unwrap().push(request);
            let reply = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
            socket.shutdown().await.ok();
        }
    });
    (url, requests)
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Value {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = socket.read(&mut chunk).await.unwrap();
        buf.extend_from_slice(&chunk[..n]);
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&buf[..end]).to_ascii_lowercase();
            let length: usize = headers
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .map(|v| v.trim().parse().unwrap())
                .unwrap_or(0);
            while buf.len() < end + 4 + length {
                let n = socket.read(&mut chunk).await.unwrap();
                buf.extend_from_slice(&chunk[..n]);
            }
            return serde_json::from_slice(&buf[end + 4..end + 4 + length]).unwrap();
        }
    }
}

/// An SSE response containing one tool call.
fn tool_call(id: &str, name: &str, input: Value) -> String {
    sse(&[
        json!({"type":"message_start","message":{"usage":{"input_tokens":1}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":id,"name":name,"input":{}}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":input.to_string()}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":1}}),
        json!({"type":"message_stop"}),
    ])
}

/// An SSE response containing only text.
fn text(text: &str) -> String {
    sse(&[
        json!({"type":"message_start","message":{"usage":{"input_tokens":1}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}),
        json!({"type":"message_stop"}),
    ])
}

fn sse(events: &[Value]) -> String {
    events
        .iter()
        .map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap()))
        .collect()
}

fn project(root: &Path) {
    fs::create_dir_all(root.join(".agents")).unwrap();
    fs::write(
        root.join(".agents/modes.toml"),
        "[commands]\nmark = \"touch ran.txt\"\n",
    )
    .unwrap();
}

fn start(url: &str, root: &Path, mode: &str, review: ReviewPolicy) -> AgentHandle {
    let spec = ProviderSpec {
        id: "fake".into(),
        backend: SpecBackend::Http(HttpProvider {
            api: HttpApi::Anthropic,
            base_url: url.into(),
            auth: Auth::None,
            models: vec!["test-model".into()],
            default_model: None,
            headers: vec![],
            capabilities: Capabilities {
                tools: true,
                streaming: true,
                prompt_caching: false,
                reasoning: false,
            },
        }),
    };
    let config = cradle_config::load(root, SourceKind::Directory).unwrap();
    let modes = ModeSet::resolve(Some(&config.modes), None, ClashChoice::Override).unwrap();
    cradle_core::spawn(AgentSetup {
        backend: Backend::new(&spec, None).unwrap(),
        guard: PathGuard::new(root).unwrap(),
        project: Some(config),
        modes,
        mode: mode.into(),
        review,
        history: vec![],
        session: None,
        data_dir: None,
    })
}

/// Run one turn, answering approvals with `approve`. Returns whether an approval was asked.
async fn turn(handle: &mut AgentHandle, prompt: &str, approve: Approval) -> bool {
    handle
        .commands
        .send(AgentCommand::Send(prompt.into()))
        .unwrap();
    let mut asked = false;
    while let Some(event) = handle.events.recv().await {
        match event {
            AgentEvent::Approval(req) => {
                asked = true;
                req.reply.send(approve.clone()).unwrap();
            }
            AgentEvent::TurnFinished => return asked,
            _ => {}
        }
    }
    panic!("agent stopped");
}

/// The tool_result content the agent sent back in request `i`.
fn tool_result(requests: &Mutex<Vec<Value>>, i: usize) -> (String, bool) {
    let requests = requests.lock().unwrap();
    let messages = requests[i]["messages"].as_array().unwrap();
    let block = &messages.last().unwrap()["content"][0];
    assert_eq!(block["type"], "tool_result");
    (
        block["content"].as_str().unwrap().to_owned(),
        block["is_error"].as_bool().unwrap(),
    )
}

#[tokio::test]
async fn plan_mode_refuses_writes() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    let (url, requests) = fake_server(vec![
        tool_call(
            "t1",
            "write_file",
            json!({"path": "evil.txt", "content": "x"}),
        ),
        text("ok"),
    ])
    .await;
    let mut agent = start(&url, tmp.path(), "plan", ReviewPolicy::User);

    let asked = turn(&mut agent, "write a file", Approval::Yes).await;

    assert!(!asked);
    assert!(!tmp.path().join("evil.txt").exists());
    let (content, is_error) = tool_result(&requests, 1);
    assert!(is_error && content.contains("read-only"), "{content}");
    // The mode was announced to the model as a system message.
    let first = &requests.lock().unwrap()[0];
    assert_eq!(first["messages"][1]["role"], "system");
}

#[tokio::test]
async fn declined_command_does_not_run() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    let (url, requests) = fake_server(vec![
        tool_call(
            "t1",
            "run_command",
            json!({"name": "mark", "reason": "test"}),
        ),
        text("ok"),
    ])
    .await;
    let mut agent = start(&url, tmp.path(), "build", ReviewPolicy::User);

    let asked = turn(&mut agent, "run it", Approval::No).await;

    assert!(asked);
    assert!(!tmp.path().join("ran.txt").exists());
    let (content, is_error) = tool_result(&requests, 1);
    assert!(is_error && content.contains("declined"), "{content}");
}

#[tokio::test]
async fn approved_command_runs() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    let (url, requests) = fake_server(vec![
        tool_call(
            "t1",
            "run_command",
            json!({"name": "mark", "reason": "test"}),
        ),
        text("ok"),
    ])
    .await;
    let mut agent = start(&url, tmp.path(), "build", ReviewPolicy::User);

    assert!(turn(&mut agent, "run it", Approval::Yes).await);
    assert!(tmp.path().join("ran.txt").exists());
    let (content, is_error) = tool_result(&requests, 1);
    assert!(!is_error && content.starts_with("exit code 0"), "{content}");
}

#[tokio::test]
async fn reviewer_approves_safe_command_without_asking() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    let (url, requests) = fake_server(vec![
        tool_call(
            "t1",
            "run_command",
            json!({"name": "mark", "reason": "test"}),
        ),
        text(r#"{"verdict": "safe", "reason": "touches a file in the project"}"#),
        text("done"),
    ])
    .await;
    let mut agent = start(&url, tmp.path(), "build", ReviewPolicy::Subagent);

    let asked = turn(&mut agent, "run it", Approval::No).await;

    assert!(!asked, "a safe verdict needs no user approval");
    assert!(tmp.path().join("ran.txt").exists());
    // The reviewer is a fresh call: no tools, no history, just the review request.
    let review = &requests.lock().unwrap()[1];
    assert!(review.get("tools").is_none());
    assert_eq!(review["messages"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn reviewer_is_skipped_after_protected_file_changes() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    let (url, requests) = fake_server(vec![
        tool_call(
            "t1",
            "write_file",
            json!({"path": "build.rs", "content": "fn main() {}"}),
        ),
        tool_call(
            "t2",
            "run_command",
            json!({"name": "mark", "reason": "build"}),
        ),
        text("done"),
    ])
    .await;
    let mut agent = start(&url, tmp.path(), "build", ReviewPolicy::Subagent);

    // Approves both the build.rs write and the command; no reviewer request is made.
    assert!(turn(&mut agent, "go", Approval::Yes).await);
    assert!(tmp.path().join("build.rs").exists());
    assert!(tmp.path().join("ran.txt").exists());
    assert_eq!(requests.lock().unwrap().len(), 3, "no reviewer call");
}

#[tokio::test]
async fn build_mode_asks_before_writing_and_always_allows_the_rest() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    let (url, _requests) = fake_server(vec![
        tool_call("t1", "write_file", json!({"path": "a.txt", "content": "a"})),
        text("ok"),
        tool_call("t2", "write_file", json!({"path": "b.txt", "content": "b"})),
        text("ok"),
    ])
    .await;
    let mut agent = start(&url, tmp.path(), "build", ReviewPolicy::User);

    assert!(turn(&mut agent, "write a", Approval::Always).await);
    assert!(tmp.path().join("a.txt").exists());
    assert!(
        !turn(&mut agent, "write b", Approval::No).await,
        "allowed for the session"
    );
    assert!(tmp.path().join("b.txt").exists());
}

#[tokio::test]
async fn read_files_returns_every_file_and_reports_failures() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    fs::write(tmp.path().join("a.txt"), "alpha\n").unwrap();
    fs::write(tmp.path().join("b.txt"), "beta\n").unwrap();
    let (url, requests) = fake_server(vec![
        tool_call(
            "t1",
            "read_files",
            json!({"files": [{"path": "a.txt"}, {"path": "nope.txt"}, {"path": "b.txt"}]}),
        ),
        tool_call("t2", "read_files", json!({"files": [{"path": "nope.txt"}]})),
        text("ok"),
    ])
    .await;
    let mut agent = start(&url, tmp.path(), "plan", ReviewPolicy::User);

    agent
        .commands
        .send(AgentCommand::Send("read".into()))
        .unwrap();
    let mut finished = Vec::new();
    while let Some(event) = agent.events.recv().await {
        match event {
            AgentEvent::ToolFinished { ok, failed, .. } => finished.push((ok, failed)),
            AgentEvent::TurnFinished => break,
            _ => {}
        }
    }

    let (content, is_error) = tool_result(&requests, 1);
    assert!(!is_error, "{content}");
    assert!(
        content.contains("==> a.txt <==\n     1\talpha"),
        "{content}"
    );
    assert!(content.contains("==> b.txt <==\n     1\tbeta"), "{content}");
    assert!(content.contains("==> nope.txt <==\nerror: "), "{content}");
    let (content, is_error) = tool_result(&requests, 2);
    assert!(is_error, "every file failed: {content}");

    assert_eq!(finished.len(), 2);
    assert!(finished[0].0);
    assert_eq!(finished[0].1.len(), 1);
    assert!(finished[0].1[0].starts_with("nope.txt: "));
    assert!(!finished[1].0);
}

#[tokio::test]
async fn requested_changes_reach_the_model() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    let (url, requests) = fake_server(vec![
        tool_call("t1", "write_file", json!({"path": "a.txt", "content": "a"})),
        text("ok"),
    ])
    .await;
    let mut agent = start(&url, tmp.path(), "build", ReviewPolicy::User);

    let feedback = Approval::Change("call it b.txt".into());
    assert!(turn(&mut agent, "write a", feedback).await);
    assert!(!tmp.path().join("a.txt").exists());
    let (content, is_error) = tool_result(&requests, 1);
    assert!(is_error && content.contains("call it b.txt"), "{content}");
}

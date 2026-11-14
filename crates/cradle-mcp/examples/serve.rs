//! Manual testing: run Cradle's MCP server with one echo tool and log every call.
//! `cargo run -p cradle-mcp --example serve` prints the URL and token, then serves.

use serde_json::json;
use tokio::sync::mpsc;

#[tokio::main]
async fn main() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let tools = vec![
        cradle_mcp::McpTool {
            name: "read_file".into(),
            description: "Read a file in the project.".into(),
            input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}),
            read_only: true,
        },
        cradle_mcp::McpTool {
            name: "write_file".into(),
            description: "Write a file in the project.".into(),
            input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}, "content": {"type": "string"}}, "required": ["path", "content"]}),
            read_only: false,
        },
    ];
    let server = cradle_mcp::start(tools, tx).await.unwrap();
    println!("{} {}", server.url(), server.token());
    while let Some(call) = rx.recv().await {
        eprintln!("tools/call {} {}", call.name, call.arguments);
        let _ = call.reply.send(cradle_mcp::ToolReply {
            text: "The secret word is pineapple.".into(),
            is_error: false,
        });
    }
}

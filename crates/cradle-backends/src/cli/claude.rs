//! Claude Code (`claude -p`) as an agent backend, using the user's Claude login.
//!
//! - `--tools ""` turns off every built-in tool; the only tools are Cradle's, over MCP.
//! - `--strict-mcp-config` ignores the user's other MCP servers.
//! - `--setting-sources user` skips project `.claude/` settings, whose hooks could run
//!   commands that Cradle never sees.
//! - `ANTHROPIC_API_KEY` is removed from the child's environment so the subscription
//!   login is used rather than API billing.

use std::io::Write;
use std::path::Path;

use serde_json::{Value, json};
use tokio::process::Command;

use super::{AgentOutcome, AgentTurn, MCP_NAME, TOOL_TIMEOUT_SECS, run_jsonl, u64_at};
use crate::{BackendError, StreamEvent, Usage};

pub(crate) struct Client {
    binary: String,
    model: Option<String>,
}

impl Client {
    pub(crate) fn new(binary: String, model: Option<String>) -> Self {
        Client { binary, model }
    }

    /// Run one turn of the conversation with Cradle's tools available.
    pub(crate) async fn turn(
        &self,
        turn: &AgentTurn,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<AgentOutcome, BackendError> {
        // The token goes in a private temp file, not on the command line where `ps` shows it.
        let config = json!({
            "mcpServers": {
                MCP_NAME: {
                    "type": "http",
                    "url": turn.mcp.url,
                    "headers": {"Authorization": format!("Bearer {}", turn.mcp.token)},
                }
            }
        });
        let mut file = tempfile::Builder::new()
            .prefix("cradle-mcp-")
            .suffix(".json")
            .tempfile()
            .map_err(|e| BackendError::Cli(format!("cannot write MCP config: {e}")))?;
        file.write_all(config.to_string().as_bytes())
            .map_err(|e| BackendError::Cli(format!("cannot write MCP config: {e}")))?;

        let mut command = self.command(&turn.system, &turn.root);
        command
            .arg("--mcp-config")
            .arg(file.path())
            .arg("--allowedTools")
            .arg(format!("mcp__{MCP_NAME}"))
            .env("MCP_TOOL_TIMEOUT", (TOOL_TIMEOUT_SECS * 1000).to_string());
        if let Some(id) = &turn.resume {
            command.arg("--resume").arg(id);
        }

        let mut parser = Parser::default();
        run_jsonl(command, Some(turn.prompt.clone()), |line| {
            parser.handle(&line, true, on_event)
        })
        .await?;
        drop(file);
        parser.finish(on_event)
    }

    /// A one-off request with no tools and no saved session (used by the reviewer).
    pub(crate) async fn complete(
        &self,
        system: &str,
        prompt: &str,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<AgentOutcome, BackendError> {
        let mut command = self.command(system, &std::env::temp_dir());
        command.arg("--no-session-persistence");
        let mut parser = Parser::default();
        run_jsonl(command, Some(prompt.to_owned()), |line| {
            parser.handle(&line, false, on_event)
        })
        .await?;
        parser.finish(on_event)
    }

    fn command(&self, system: &str, cwd: &Path) -> Command {
        let mut command = Command::new(&self.binary);
        command
            .current_dir(cwd)
            .args([
                "-p",
                "--output-format",
                "stream-json",
                "--verbose",
                "--include-partial-messages",
                "--tools",
                "",
                "--strict-mcp-config",
                "--setting-sources",
                "user",
                "--disable-slash-commands",
                "--system-prompt",
            ])
            .arg(system)
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("CLAUDECODE");
        if let Some(model) = &self.model {
            command.arg("--model").arg(model);
        }
        command
    }
}

/// Reads Claude Code's `stream-json` events.
#[derive(Default)]
struct Parser {
    session_id: Option<String>,
    streamed: bool,
    /// Text from complete assistant messages, used if no partial events arrived.
    fallback: String,
    result: Option<String>,
    error: Option<String>,
    usage: Usage,
}

impl Parser {
    /// Handle one event. Returns `true` at the final `result` event.
    fn handle(
        &mut self,
        line: &Value,
        expect_mcp: bool,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<bool, BackendError> {
        if let Some(id) = line.get("session_id").and_then(Value::as_str) {
            self.session_id = Some(id.to_owned());
        }
        match line.get("type").and_then(Value::as_str).unwrap_or("") {
            "system" if line.get("subtype").and_then(Value::as_str) == Some("init") => {
                if expect_mcp {
                    let status = line
                        .get("mcp_servers")
                        .and_then(Value::as_array)
                        .and_then(|servers| {
                            servers
                                .iter()
                                .find(|s| s.get("name").and_then(Value::as_str) == Some(MCP_NAME))
                        })
                        .and_then(|s| s.get("status"))
                        .and_then(Value::as_str);
                    if let Some(status) = status
                        && status != "connected"
                    {
                        return Err(BackendError::Cli(format!(
                            "Claude Code could not connect to Cradle's tool server ({status})"
                        )));
                    }
                }
            }
            "stream_event" => {
                let event = line.get("event").cloned().unwrap_or_default();
                if event.get("type").and_then(Value::as_str) == Some("content_block_delta") {
                    let delta = event.get("delta").cloned().unwrap_or_default();
                    match delta.get("type").and_then(Value::as_str) {
                        Some("text_delta") => {
                            if let Some(t) = delta.get("text").and_then(Value::as_str) {
                                self.streamed = true;
                                on_event(StreamEvent::TextDelta(t.to_owned()));
                            }
                        }
                        Some("thinking_delta") => {
                            if let Some(t) = delta.get("thinking").and_then(Value::as_str) {
                                on_event(StreamEvent::ThinkingDelta(t.to_owned()));
                            }
                        }
                        _ => {}
                    }
                }
            }
            "assistant" => {
                for block in line
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if block.get("type").and_then(Value::as_str) == Some("text")
                        && let Some(t) = block.get("text").and_then(Value::as_str)
                    {
                        self.fallback.push_str(t);
                    }
                }
            }
            "result" => {
                let text = line
                    .get("result")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let failed = line.get("is_error").and_then(Value::as_bool) == Some(true)
                    || line.get("subtype").and_then(Value::as_str) != Some("success");
                if failed {
                    let subtype = line
                        .get("subtype")
                        .and_then(Value::as_str)
                        .unwrap_or("error");
                    self.error = Some(if text.is_empty() {
                        subtype.to_owned()
                    } else {
                        text.clone()
                    });
                }
                if let Some(usage) = line.get("usage") {
                    self.usage = Usage {
                        input_tokens: u64_at(usage, "input_tokens"),
                        output_tokens: u64_at(usage, "output_tokens"),
                        cache_read_tokens: u64_at(usage, "cache_read_input_tokens"),
                        cache_write_tokens: u64_at(usage, "cache_creation_input_tokens"),
                    };
                }
                self.result = Some(text);
                return Ok(true);
            }
            _ => {}
        }
        Ok(false)
    }

    fn finish(
        self,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<AgentOutcome, BackendError> {
        if let Some(error) = self.error {
            return Err(BackendError::Cli(format!("Claude Code: {error}")));
        }
        let text = self
            .result
            .filter(|t| !t.is_empty())
            .unwrap_or(self.fallback);
        if !self.streamed && !text.is_empty() {
            on_event(StreamEvent::TextDelta(text.clone()));
        }
        Ok(AgentOutcome {
            session_id: self.session_id,
            text,
            usage: self.usage,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(
        lines: &[Value],
        expect_mcp: bool,
    ) -> (Result<AgentOutcome, BackendError>, Vec<StreamEvent>) {
        let mut events = Vec::new();
        let mut sink = |e: StreamEvent| events.push(e);
        let mut parser = Parser::default();
        for line in lines {
            match parser.handle(line, expect_mcp, &mut sink) {
                Ok(true) => break,
                Ok(false) => {}
                Err(e) => return (Err(e), events),
            }
        }
        let outcome = parser.finish(&mut sink);
        (outcome, events)
    }

    #[test]
    fn streams_text_and_reads_result() {
        let (outcome, events) = run(
            &[
                json!({"type":"system","subtype":"init","session_id":"s1","mcp_servers":[{"name":"cradle","status":"connected"}]}),
                json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hel"}}}),
                json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"lo"}}}),
                json!({"type":"assistant","message":{"content":[{"type":"text","text":"Hello"}]}}),
                json!({"type":"result","subtype":"success","is_error":false,"result":"Hello","session_id":"s1","usage":{"input_tokens":5,"output_tokens":2}}),
            ],
            true,
        );
        let outcome = outcome.unwrap();
        assert_eq!(outcome.session_id.as_deref(), Some("s1"));
        assert_eq!(outcome.text, "Hello");
        assert_eq!(outcome.usage.output_tokens, 2);
        assert_eq!(
            events,
            [
                StreamEvent::TextDelta("Hel".into()),
                StreamEvent::TextDelta("lo".into())
            ],
            "text is not emitted twice"
        );
    }

    #[test]
    fn falls_back_to_full_messages() {
        let (outcome, events) = run(
            &[
                json!({"type":"assistant","message":{"content":[{"type":"text","text":"Hi"}]}}),
                json!({"type":"result","subtype":"success","is_error":false,"result":"","session_id":"s2"}),
            ],
            false,
        );
        assert_eq!(outcome.unwrap().text, "Hi");
        assert_eq!(events, [StreamEvent::TextDelta("Hi".into())]);
    }

    #[test]
    fn reports_failures() {
        let (outcome, _) = run(
            &[
                json!({"type":"system","subtype":"init","mcp_servers":[{"name":"cradle","status":"failed"}]}),
            ],
            true,
        );
        assert!(
            outcome
                .unwrap_err()
                .to_string()
                .contains("could not connect")
        );

        let (outcome, _) = run(
            &[json!({"type":"result","subtype":"error_max_turns","is_error":true})],
            false,
        );
        assert!(outcome.unwrap_err().to_string().contains("error_max_turns"));
    }
}

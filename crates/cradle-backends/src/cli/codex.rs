//! Codex (`codex exec`) as an agent backend, using the user's ChatGPT login.
//!
//! Checked against codex-cli 0.169.0 (`codex exec --help`, `codex features list`).
//!
//! - Every feature that runs commands or reaches out is disabled ([`DISABLED_FEATURES`]):
//!   besides `shell_tool`, Codex also has `unified_exec`, computer use and browsers.
//! - The code-mode host stays on: Codex calls MCP tools from inside it, and its `exec`
//!   cannot import filesystem or network modules (checked: "unsupported import in exec").
//! - `default_tools_approval_mode = "approve"` for Cradle's server: Cradle approves each
//!   call itself, so Codex should not refuse it.
//! - `sandbox_mode = "read-only"` with `approval_policy = "never"` makes the OS refuse any
//!   write Codex attempts on its own; writes go through Cradle's tools.
//! - `--ignore-user-config` keeps the user's own MCP servers and settings out.
//! - Cradle's tools come from an HTTP MCP server; the token is passed in an env var.
//! - `OPENAI_API_KEY` is removed so the ChatGPT login is used rather than API billing.

use std::path::Path;

use serde_json::Value;
use tokio::process::Command;

use super::{AgentOutcome, AgentTurn, MCP_NAME, TOOL_TIMEOUT_SECS, run_jsonl, u64_at};
use crate::{BackendError, StreamEvent, Usage};

const TOKEN_ENV: &str = "CRADLE_MCP_TOKEN";

/// Codex features that run code, browse, or act outside Cradle's tools.
const DISABLED_FEATURES: &[&str] = &[
    "shell_tool",
    "unified_exec",
    "unified_exec_tty",
    "computer_use",
    "browser_use",
    "browser_use_external",
    "in_app_browser",
    "apps",
    "multi_agent",
    "image_generation",
];

pub(crate) struct Client {
    binary: String,
    model: Option<String>,
}

impl Client {
    pub(crate) fn new(binary: String, model: Option<String>) -> Self {
        Client { binary, model }
    }

    pub(crate) async fn turn(
        &self,
        turn: &AgentTurn,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<AgentOutcome, BackendError> {
        let mut command = self.command(&turn.root, turn.resume.is_some());
        for setting in [
            format!("mcp_servers.{MCP_NAME}.url=\"{}\"", turn.mcp.url),
            format!("mcp_servers.{MCP_NAME}.bearer_token_env_var=\"{TOKEN_ENV}\""),
            format!("mcp_servers.{MCP_NAME}.tool_timeout_sec={TOOL_TIMEOUT_SECS}"),
            format!("mcp_servers.{MCP_NAME}.startup_timeout_sec=30"),
            // Cradle approves calls itself; Codex must not block them a second time.
            format!("mcp_servers.{MCP_NAME}.default_tools_approval_mode=\"approve\""),
        ] {
            command.arg("-c").arg(setting);
        }
        command.env(TOKEN_ENV, &turn.mcp.token);

        // Codex has no system prompt flag, so the first turn carries the instructions.
        let prompt = match &turn.resume {
            Some(id) => {
                command.arg(id);
                turn.prompt.clone()
            }
            None => with_instructions(&turn.system, &turn.prompt),
        };
        command.arg(prompt);

        let mut parser = Parser::default();
        run_jsonl(command, None, |line| parser.handle(&line, on_event)).await?;
        parser.finish()
    }

    /// A one-off request with no tools and nothing saved (used by the reviewer).
    pub(crate) async fn complete(
        &self,
        system: &str,
        prompt: &str,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<AgentOutcome, BackendError> {
        let mut command = self.command(&std::env::temp_dir(), false);
        command
            .arg("--ephemeral")
            .arg(with_instructions(system, prompt));
        let mut parser = Parser::default();
        run_jsonl(command, None, |line| parser.handle(&line, on_event)).await?;
        parser.finish()
    }

    /// `codex exec [resume] <options>`; the session id and prompt are added by the caller.
    /// Options go after `resume` because it does not accept all of `exec`'s (no `--sandbox`).
    fn command(&self, cwd: &Path, resume: bool) -> Command {
        let mut command = Command::new(&self.binary);
        command.current_dir(cwd).arg("exec");
        if resume {
            command.arg("resume");
        }
        command
            .args([
                "--json",
                "--skip-git-repo-check",
                "--ignore-user-config",
                "-c",
                "sandbox_mode=\"read-only\"",
                "-c",
                "approval_policy=\"never\"",
            ])
            .env_remove("OPENAI_API_KEY");
        for feature in DISABLED_FEATURES {
            command.arg("--disable").arg(feature);
        }
        if let Some(model) = &self.model {
            command.arg("-m").arg(model);
        }
        command
    }
}

fn with_instructions(system: &str, prompt: &str) -> String {
    format!("<harness_instructions>\n{system}\n</harness_instructions>\n\n{prompt}")
}

/// Reads `codex exec --json` events.
#[derive(Default)]
struct Parser {
    thread_id: Option<String>,
    text: String,
    error: Option<String>,
    usage: Usage,
}

impl Parser {
    fn handle(
        &mut self,
        line: &Value,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<bool, BackendError> {
        match line.get("type").and_then(Value::as_str).unwrap_or("") {
            "thread.started" => {
                self.thread_id = line
                    .get("thread_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            "item.completed" => {
                let item = line.get("item").cloned().unwrap_or_default();
                let text = item
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                match item.get("type").and_then(Value::as_str) {
                    Some("agent_message") if !text.is_empty() => {
                        if !self.text.is_empty() {
                            self.text.push_str("\n\n");
                            on_event(StreamEvent::TextDelta("\n\n".into()));
                        }
                        self.text.push_str(&text);
                        on_event(StreamEvent::TextDelta(text));
                    }
                    Some("reasoning") if !text.is_empty() => {
                        on_event(StreamEvent::ThinkingDelta(format!("{text}\n")));
                    }
                    _ => {}
                }
            }
            "turn.completed" => {
                if let Some(usage) = line.get("usage") {
                    self.usage = Usage {
                        input_tokens: u64_at(usage, "input_tokens"),
                        output_tokens: u64_at(usage, "output_tokens"),
                        cache_read_tokens: u64_at(usage, "cached_input_tokens"),
                        cache_write_tokens: 0,
                    };
                }
                return Ok(true);
            }
            "turn.failed" => {
                let message = line
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("turn failed");
                return Err(BackendError::Cli(format!("Codex: {message}")));
            }
            "error" => {
                self.error = line
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            _ => {}
        }
        Ok(false)
    }

    fn finish(self) -> Result<AgentOutcome, BackendError> {
        if self.text.is_empty()
            && let Some(error) = self.error
        {
            return Err(BackendError::Cli(format!("Codex: {error}")));
        }
        Ok(AgentOutcome {
            session_id: self.thread_id,
            text: self.text,
            usage: self.usage,
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn reads_documented_events() {
        let mut events = Vec::new();
        let mut sink = |e: StreamEvent| events.push(e);
        let mut parser = Parser::default();
        let lines = [
            json!({"type":"thread.started","thread_id":"t1"}),
            json!({"type":"turn.started"}),
            json!({"type":"item.completed","item":{"id":"i0","type":"reasoning","text":"Looking."}}),
            json!({"type":"item.completed","item":{"id":"i1","type":"agent_message","text":"Done."}}),
            json!({"type":"turn.completed","usage":{"input_tokens":10,"cached_input_tokens":4,"output_tokens":3}}),
        ];
        let mut finished = false;
        for line in &lines {
            if parser.handle(line, &mut sink).unwrap() {
                finished = true;
                break;
            }
        }
        assert!(finished);
        let outcome = parser.finish().unwrap();
        assert_eq!(outcome.session_id.as_deref(), Some("t1"));
        assert_eq!(outcome.text, "Done.");
        assert_eq!(outcome.usage.cache_read_tokens, 4);
        assert!(events.contains(&StreamEvent::ThinkingDelta("Looking.\n".into())));
    }

    #[test]
    fn turn_failed_is_an_error() {
        let mut parser = Parser::default();
        let err = parser
            .handle(
                &json!({"type":"turn.failed","error":{"message":"usage limit"}}),
                &mut |_| {},
            )
            .unwrap_err();
        assert!(err.to_string().contains("usage limit"));
    }
}

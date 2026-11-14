//! Agent backends that drive an installed CLI (Claude Code, Codex) with all of its
//! built-in tools turned off. The CLI runs the agent loop; its only tools are Cradle's,
//! served over MCP, so every call still passes through Cradle's checks and approvals.

pub(crate) mod claude;
pub(crate) mod codex;

use std::path::PathBuf;
use std::process::{Command as StdCommand, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::{BackendError, Usage};

/// One user turn handed to a CLI.
#[derive(Debug, Clone)]
pub struct AgentTurn {
    /// Cradle's system prompt.
    pub system: String,
    /// The user's message, plus any harness notices (mode changes).
    pub prompt: String,
    /// The CLI's own session id from the previous turn, to continue the conversation.
    pub resume: Option<String>,
    /// Where Cradle's MCP tool server is listening.
    pub mcp: McpEndpoint,
    /// The project root, used as the CLI's working directory.
    pub root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct McpEndpoint {
    pub url: String,
    pub token: String,
}

/// The result of a CLI turn.
#[derive(Debug, Clone, Default)]
pub struct AgentOutcome {
    /// The CLI's session id, to resume next turn.
    pub session_id: Option<String>,
    /// The final reply text.
    pub text: String,
    pub usage: Usage,
}

/// Cradle's MCP server name, so tools appear as `mcp__cradle__read_file` and similar.
pub(crate) const MCP_NAME: &str = "cradle";
/// Approvals can take a while; tool calls must not time out under the user.
pub(crate) const TOOL_TIMEOUT_SECS: u64 = 24 * 60 * 60;
const MAX_STDERR: usize = 8 * 1024;

/// `<binary> --version`, or `None` if it is missing or does not answer within 3 seconds.
/// Returns the first word that starts with a digit ("2.1.311 (Claude Code)" → "2.1.311").
pub fn installed_version(binary: &str) -> Option<String> {
    let mut child = StdCommand::new(binary)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if Instant::now() > deadline => {
                let _ = child.kill();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    let output = child.wait_with_output().ok()?;
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .map(str::to_owned)
}

/// Check the installed CLI against the provider's semver range.
pub(crate) fn check_version(
    provider: &str,
    binary: &str,
    range: Option<&str>,
) -> Result<String, BackendError> {
    let version = installed_version(binary).ok_or_else(|| {
        BackendError::Cli(format!("`{binary}` is not installed (or not on PATH)"))
    })?;
    if let Some(range) = range {
        let req = semver::VersionReq::parse(range)
            .map_err(|e| BackendError::Cli(format!("bad version range `{range}`: {e}")))?;
        let parsed = semver::Version::parse(&version);
        if !parsed.as_ref().is_ok_and(|v| req.matches(v)) {
            return Err(BackendError::Cli(format!(
                "`{binary}` {version} is outside the supported range {range} for provider \
                 `{provider}`; update providers.toml once you have checked it works"
            )));
        }
    }
    Ok(version)
}

/// Spawn `command`, write `stdin`, and feed each JSON line of stdout to `on_line` until it
/// returns `true` (done). The child is killed if this future is dropped.
pub(crate) async fn run_jsonl(
    mut command: Command,
    stdin: Option<String>,
    mut on_line: impl FnMut(Value) -> Result<bool, BackendError>,
) -> Result<(), BackendError> {
    command
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let program = command
        .as_std()
        .get_program()
        .to_string_lossy()
        .into_owned();
    let mut child = command
        .spawn()
        .map_err(|e| BackendError::Cli(format!("cannot start `{program}`: {e}")))?;

    if let Some(input) = stdin
        && let Some(mut pipe) = child.stdin.take()
    {
        pipe.write_all(input.as_bytes())
            .await
            .map_err(|e| BackendError::Cli(format!("cannot write to `{program}`: {e}")))?;
        drop(pipe);
    }
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf).await;
        let text = String::from_utf8_lossy(&buf).into_owned();
        let mut start = text.len().saturating_sub(MAX_STDERR);
        while !text.is_char_boundary(start) {
            start += 1;
        }
        text[start..].to_owned()
    });

    let stdout = child.stdout.take().expect("stdout is piped");
    let mut lines = BufReader::new(stdout).lines();
    let mut done = false;
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|e| BackendError::Cli(format!("reading `{program}` output: {e}")))?
    {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue; // not every line is an event (warnings, banners)
        };
        if on_line(value)? {
            done = true;
            break;
        }
    }

    let status = child
        .wait()
        .await
        .map_err(|e| BackendError::Cli(format!("`{program}` failed: {e}")))?;
    let stderr = stderr_task.await.unwrap_or_default();
    if done {
        return Ok(());
    }
    let detail = stderr.trim();
    Err(BackendError::Cli(if status.success() {
        format!("`{program}` exited without a result. {detail}")
    } else {
        format!("`{program}` exited with {status}. {detail}")
    }))
}

pub(crate) fn u64_at(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

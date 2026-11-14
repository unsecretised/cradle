//! Runs `[commands]` entries and `git diff` with a scrubbed environment.
//!
//! There is no shell: the command line from project config is split into words and the
//! program is started directly. The child sees only an allowlist of environment variables,
//! so API keys and other secrets in Cradle's environment never reach build scripts.
//!
//! This is not a sandbox yet. See `.agents/roadmap/FEATURE_SANDBOX.md`.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use crate::ToolError;
use crate::hygiene;

/// Variables passed through to commands. Everything else is dropped.
const ENV_ALLOWLIST: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TERM",
    "TMPDIR",
    "TZ",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "RUSTUP_TOOLCHAIN",
];

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_STREAM_CHARS: usize = 20_000;

/// Split a `[commands]` value into program and arguments.
pub fn split(command_line: &str) -> Result<Vec<String>, ToolError> {
    match shlex::split(command_line) {
        Some(words) if !words.is_empty() => Ok(words),
        _ => Err(ToolError::new(format!(
            "cannot parse command `{command_line}`"
        ))),
    }
}

/// Run `argv` in `cwd` and return a report with exit status and (cleaned) output.
/// The child is killed if this future is dropped.
pub async fn run(argv: &[String], cwd: &Path, timeout: Duration) -> Result<String, ToolError> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| ToolError::new("empty command"))?;

    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for var in ENV_ALLOWLIST {
        if let Some(value) = std::env::var_os(var) {
            command.env(var, value);
        }
    }
    command
        .env("NO_COLOR", "1")
        .env("CARGO_TERM_COLOR", "never")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat");

    let child = command
        .spawn()
        .map_err(|e| ToolError::new(format!("cannot start `{program}`: {e}")))?;

    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(result) => result.map_err(|e| ToolError::new(format!("`{program}` failed: {e}")))?,
        Err(_) => {
            return Err(ToolError::new(format!(
                "`{program}` timed out after {}s and was killed",
                timeout.as_secs()
            )));
        }
    };

    let status = match output.status.code() {
        Some(code) => format!("exit code {code}"),
        None => "terminated by signal".to_owned(),
    };
    let mut report = status;
    for (label, bytes) in [("stdout", &output.stdout), ("stderr", &output.stderr)] {
        let text = String::from_utf8_lossy(bytes);
        if !text.trim().is_empty() {
            report.push_str(&format!(
                "\n--- {label} ---\n{}",
                cap(&hygiene::clean(&text))
            ));
        }
    }
    Ok(report)
}

/// `git diff` with external diff drivers and textconv filters disabled, since both can
/// be configured to run arbitrary programs.
pub async fn git_diff(root: &Path, path: Option<&str>, staged: bool) -> Result<String, ToolError> {
    let mut argv: Vec<String> = [
        "git",
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    if staged {
        argv.push("--cached".into());
    }
    if let Some(path) = path {
        argv.push("--".into());
        argv.push(path.to_owned());
    }
    run(&argv, root, Duration::from_secs(60)).await
}

/// Keep the head and the tail of long output, where errors usually are.
fn cap(text: &str) -> String {
    let count = text.chars().count();
    if count <= MAX_STREAM_CHARS {
        return text.to_owned();
    }
    let head: String = text.chars().take(MAX_STREAM_CHARS / 4).collect();
    let tail: String = text
        .chars()
        .skip(count - MAX_STREAM_CHARS * 3 / 4)
        .collect();
    format!(
        "{head}\n... ({} characters omitted) ...\n{tail}",
        count - MAX_STREAM_CHARS
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_quoted_words() {
        assert_eq!(
            split(r#"cargo test -- "a b""#).unwrap(),
            ["cargo", "test", "--", "a b"]
        );
        assert!(split("").is_err());
        assert!(split("unterminated \"quote").is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runs_without_secrets_in_env() {
        // SAFETY: tests in this module do not read this variable concurrently.
        unsafe { std::env::set_var("CRADLE_TEST_SECRET", "sk-should-not-leak") };
        let dir = tempfile::tempdir().unwrap();
        let report = run(&["env".into()], dir.path(), Duration::from_secs(10))
            .await
            .unwrap();
        assert!(report.starts_with("exit code 0"));
        assert!(!report.contains("sk-should-not-leak"), "{report}");
        assert!(report.contains("NO_COLOR=1"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reports_failure_and_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let report = run(&["false".into()], dir.path(), Duration::from_secs(10))
            .await
            .unwrap();
        assert!(report.starts_with("exit code 1"));
        let err = run(
            &["sleep".into(), "5".into()],
            dir.path(),
            Duration::from_millis(100),
        )
        .await
        .unwrap_err();
        assert!(err.0.contains("timed out"));
    }

    #[test]
    fn caps_long_output() {
        let long = "x".repeat(MAX_STREAM_CHARS + 100);
        let capped = cap(&long);
        assert!(capped.contains("100 characters omitted"));
    }
}

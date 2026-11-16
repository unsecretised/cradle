# Session resume across backends

Resuming a session doesn't work properly with CLI backends, and the backend itself isn't
remembered. This finishes the "Not built" part of `features/FEATURE_SESSIONS.md`.

## Current problem

- The CLI's session id (Claude Code `session_id`, Codex thread id) is kept in memory only
  (`agent.rs` `cli_session`). After `--continue`, `/session`, a restart or a backend
  switch, the CLI starts a fresh session: the TUI shows the earlier messages, but the
  model never sees them ("starts a fresh session: the resumed messages are shown but not
  sent").
- Provider and model aren't written to the transcript, so a resumed session opens with
  whatever backend is the current default.

## Proposed

- **Transcript meta lines.** Besides messages, a transcript may hold
  `{"meta":"backend","provider","model"}` and `{"meta":"cli_session","provider","id"}`
  lines (untagged, so older transcripts still read). The last one of each kind wins.
- **Record the backend.** Write a `backend` line at the start and on every `SetBackend`.
  On resume (`--continue` or `/session`), switch back to that provider and model if
  they're available; `--provider`/`--model` still win. Otherwise, use the current one and
  show a notice. The agent asks the TUI to switch (`AgentEvent::SessionBackend`), since
  only the TUI can build a backend.
  The backend line is written lazily, before the next message, and only when it changed,
  so sessions with no messages get no meta lines.
- **Persist the CLI session id.** Write a `cli_session` line whenever
  `outcome.session_id` changes. On resume with the same CLI provider, pass it to
  `--resume` (Claude Code) or `codex exec resume` (Codex).
- **One rule for when a CLI session is usable:** the agent keeps it with its provider and
  uses it only while that provider is the current backend. At the start of each turn, a
  session for another provider is dropped: a turn on a different backend means the CLI
  hasn't seen part of the history. Switching away and back without sending anything keeps
  it.
- **Fallback when there's no usable CLI session** (different provider, a transcript from
  an HTTP backend, an old transcript with no saved id, a provider switch mid-session, or
  an expired session): the first CLI turn starts with the **full earlier conversation as
  text** in a `<previous_conversation>` block, in order: user, assistant and system
  (harness) messages verbatim; each tool call with its name and JSON input; each tool
  result in full (flagged when it was an error). Thinking and other opaque blocks are
  left out. Later turns resume the new CLI session as usual.
- **Expired sessions:** if a resumed CLI turn fails before it streams any text or makes a
  tool call, it is retried once without resuming, with the fallback, and a notice says so.
  If the retry fails too, the old session id is kept and the original error is shown, so
  a passing failure (e.g. a usage limit) doesn't lose the session. No CLI-specific error
  strings are matched.
- `cradle --resume <id>` opens a specific session directly. `<id>` is the transcript's
  file name without `.jsonl` (shown in `/session`), or a unique prefix of it. It conflicts
  with `--continue`.

## Open

- TODO: check whether HTTP backends have edge cases on resume (e.g. thinking blocks from a
  different model, history written by a CLI turn).

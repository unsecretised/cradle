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

- **Record the backend.** Write a transcript event with provider and model at the start
  and on every `SetBackend`. On resume, switch back to that provider/model if it's
  available. Otherwise, use the current one and show a notice.
- **Persist the CLI session id.** Write a transcript event (provider + CLI session id)
  whenever `outcome.session_id` changes. On resume with the same CLI provider, pass it
  to `--resume` (Claude Code) or `codex exec resume` (Codex).
- **Fallback when there's no usable CLI session** (different provider, a transcript from
  an HTTP backend, or the CLI no longer has the session): send the earlier conversation
  as context in the first turn, so the model still sees it. Applies after a backend
  switch mid-session too.
- `cradle --resume <id>` to open a specific session directly.

## Open

- Fallback format: the full transcript as text, a trimmed tail, or a model-written
  summary? How should tool calls and results be shown?
- How to detect that a CLI session has expired, and whether to fall back silently or tell
  the user.
- TODO: check whether HTTP backends have edge cases on resume (e.g. thinking blocks from a
  different model, history written by a CLI turn).

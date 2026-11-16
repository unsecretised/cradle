# Sessions

Conversations are saved so they can be resumed (`cradle-core/src/session.rs`).

- Transcripts are JSONL in
  `~/.local/share/cradle/sessions/<sha256 of project path>/<unix secs>-<pid>.jsonl`: one
  canonical message per line, plus meta lines (`{"meta":"backend",...}`,
  `{"meta":"cli_session",...}`; the last of each kind applies). Transcripts without meta
  lines still read.
- Mode changes appear as system messages. The backend (provider + model) is recorded
  before the first message it handles; a CLI's session id whenever it changes.
- `cradle --continue` (`-c`) resumes the most recent transcript; `cradle --resume <id>`
  a specific one (file name without `.jsonl`, or a unique prefix; shown in `/session`).
- `/session` (aliases `/resume`, `/sessions`) lists earlier transcripts and resumes one.
  `/clear` starts a new one.
- Resuming switches back to the conversation's provider and model (`--provider`/`--model`
  win; the saved default is not changed). If it can't be used, a notice says why.
- CLI backends: the session id is kept with its provider and used only while that
  provider is the backend; a turn on another backend drops it. With no usable session,
  the first CLI turn carries the full earlier conversation as text
  (`<previous_conversation>`: messages, tool calls and results; no thinking). A resumed
  turn that fails before any output is retried once that way; if the retry fails too,
  the old id is kept.

## Open

- Codex gets the prompt as an argument, so a very long earlier conversation can exceed
  OS argument limits (check whether `codex exec -` reads stdin).
- Check HTTP backends for resume edge cases (thinking blocks from a different model).

# Sessions

Conversations are saved so they can be resumed (`cradle-core/src/session.rs`).

- Transcripts are JSONL, one canonical message per line, in
  `~/.local/share/cradle/sessions/<sha256 of project path>/<unix secs>-<pid>.jsonl`.
- Mode changes appear in the transcript as system messages. Provider and model are not
  recorded.
- `cradle --continue` (`-c`) resumes the most recent transcript for the project.
- `/session` (aliases `/resume`, `/sessions`) lists earlier transcripts (start time, first
  user message, message count) and resumes one. `/clear` starts a new one.
- CLI backends: the CLI's own session id is kept in memory only, and a CLI turn sends only
  the new message. After resuming (or `/clear`, or a restart), the CLI starts a fresh
  session and does not see the earlier messages, though the TUI still shows them.

## Not built (from the original plan)

- `cradle --resume <id>` to open a specific session directly.
- Recording the provider and model.
- Storing the CLI's session id so a resumed session can pass that CLI's resume flag.

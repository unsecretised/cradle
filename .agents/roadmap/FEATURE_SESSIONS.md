# Sessions

## Decisions

- Transcripts are stored as JSONL in `~/.local/share/cradle/sessions/<project-hash>/<id>.jsonl`.
  Each line is one canonical message or event.
- `/resume` lists recent sessions for the project, and `cradle --resume <id>` opens one
  directly.
- CLI backends: Cradle also stores the CLI's own session id so it can pass that CLI's
  resume flag.
- The active provider, model and mode are recorded at the start and whenever they change.

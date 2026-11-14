# `.agents/` doc proposals

The agent keeps the docs up to date without changing them behind the user's back.

## Already built (see `features/FEATURE_TOOLS.md`)

- `write_file` / `edit_file` on an `.agents/` doc always shows a diff and needs the
  user's approval, whatever the mode's `file_edits` setting.
- `RULES.md`, `modes.toml`, `modes/` and `agents.toml` are refused outside scaffold mode.

## Still open

- A dedicated `propose_agents_edit(path, new_content, reason)` tool that queues proposals
  for review together, instead of approving each write as it happens. Decide whether this
  is still wanted.
- Typical uses to support: moving roadmap → features, updating `features.md`,
  `architecture.md`, `deps.md` and `TESTS.md`, writing `bugfixes/BUGFIX_*.md`.

# `.agents/` doc proposals

The agent keeps the docs up to date without writing to them directly.

## Decisions

- `propose_agents_edit(path, new_content, reason)` queues a proposal.
- The TUI shows a diff (made with `similar`). The user accepts or rejects it, and only an
  accepted proposal is written.
- Proposals are never allowed for `RULES.md`, `modes.toml` or `modes/`.
- Typical uses: moving roadmap → features, updating `features.md`, `architecture.md`,
  `deps.md` and `TESTS.md`, writing `bugfixes/BUGFIX_*.md`.

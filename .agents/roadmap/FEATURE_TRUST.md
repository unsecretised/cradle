# Trust

Anything that comes from the project's config must be approved by the user before it
applies.

## Decisions

- What counts as trusted config: `modes.toml` / `agents.toml` (including `[commands]`),
  `modes/*`, `RULES.md`, soul overrides.
- Cradle renders it as a text snapshot (including mode commands) and stores the snapshot's
  sha256 in `~/.config/cradle/trust.toml`, keyed by canonical project path. The approved
  snapshot itself is kept in `~/.config/cradle/trust/`.
- Config is reloaded before each message. A trusted hash applies silently. A new one shows
  the diff against the last approved snapshot and asks. If declined, the current config
  stays and that version is not asked about again this session.
- Until approved, Cradle runs with global config only. Project commands are unavailable.
- The `.agents/` vs `agents.toml` clash choice is stored here too.

# Trust

Anything that comes from the project's config must be approved by the user before it
applies.

## Decisions

- What counts as trusted config: `modes.toml` / `agents.toml` (including `[commands]`),
  `modes/*`, `RULES.md`, soul overrides.
- Cradle hashes it with sha256, and stores approvals in `~/.config/cradle/trust.toml`, keyed
  by canonical project path + hash.
- When the hash changes, Cradle shows a diff against the last approved version and asks
  again.
- Until approved, Cradle runs with global config only. Project commands are unavailable.
- The `.agents/` vs `agents.toml` clash choice is stored here too.

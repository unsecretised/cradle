# Project config loading

Load project knowledge from either form into one `ProjectConfig`.

## Forms

- **Directory:** `.agents/` containing `features/`, `roadmap/`, `bugfixes/`, `BUILD.md`,
  `TESTS.md`, `features.md`, `product.md`, `architecture.md`, `deps.md`, `RULES.md` and
  `modes.toml`.
- **Single file:** `agents.toml` with tables such as `[features]`, `[roadmap]`,
  `[architecture.<path>]`, plus the same `[default]`, `[extra]` and `[commands]` tables as
  `modes.toml`.

## Decisions

- If both exist: warn, ask the user which one to use, and store the choice in `trust.toml`.
  Ask again if either form changes.
- Context loading:
  - Always loaded: `product.md`, `RULES.md`, `architecture.md`, `features.md`, the active
    mode.
  - Fetched on demand via a tool: `features/*`, `roadmap/*`, `bugfixes/*`, `deps.md`,
    `TESTS.md`, `BUILD.md`.
- Mode paths are relative to `.agents/`, and are rejected if they resolve outside it (checks
  cover `..`, absolute paths and symlinks).

## Open

- The exact `agents.toml` key names for BUILD/TESTS/RULES/deps content (strings vs. tables).

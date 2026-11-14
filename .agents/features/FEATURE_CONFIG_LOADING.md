# Project config loading

`cradle-config` loads project knowledge from either form into one `ProjectConfig`.

## Forms

- **Directory:** `.agents/` with `features/`, `roadmap/`, `bugfixes/`, `skills/`,
  `BUILD.md`, `TESTS.md`, `features.md`, `product.md`, `architecture.md`, `deps.md`,
  `RULES.md`, `modes.toml` and `modes/`. Every file is optional.
- **Single file:** `agents.toml` at the root, with `[docs]` (string keys `product`,
  `rules`, `build`, `tests`, `deps`), `[features]`, `[roadmap]`, `[bugfixes]` (name =
  one-line summary), `[architecture.<path>]` (rendered as a file tree), plus the same
  `[default]`, `[extra]`, `[commands]` and `[<name>_mode.commands]` tables as `modes.toml`.
  Mode files and skills still live in `.agents/modes/` and `.agents/skills/`.

## Behaviour

- `.agents/` counts as the directory form only if it holds something other than `modes/`
  and `skills/`.
- If both forms exist, `cradle` asks on startup which to use and stores the choice in
  `trust.toml`. `cradle config` fails instead, with a hint to pass `--source dir|file`.
- Context loading (see `cradle-core/src/prompt.rs`):
  - Always in the prompt: `product.md`, `RULES.md`, `architecture.md`, `features.md`.
  - Listed for on-demand reading with `read_file`: `BUILD.md`, `TESTS.md`, `deps.md`,
    `features/*`, `roadmap/*`, `bugfixes/*`. Inline `agents.toml` entries are listed with
    their summary.
  - The active mode arrives as a system message (see `FEATURE_MODES.md`).
- Mode paths are relative to `.agents/` and rejected if they resolve outside it (`..`,
  absolute paths, symlinks).
- `cradle config` prints what would be loaded.

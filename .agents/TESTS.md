# Tests

No tests exist yet. This is the plan for them.

## Unit tests (per crate, `#[cfg(test)]`)

- **cradle-config**: parsing `.agents/` and `agents.toml` into the same `ProjectConfig`;
  detecting a clash when both are present; resolving mode paths (rejecting `..`, symlinks
  and absolute paths that escape `.agents/`); `clash_handler` override/combine/ask
  behaviour; soul merge.
- **cradle-config (trust)**: the config hash stays stable across runs, and changes when a
  trusted file changes.
- **cradle-provider-spec**: validation errors for a duplicate `id`, an unknown `kind`, a bad
  URL, `default_model` missing from `models`, a field not allowed for that kind,
  `prompt_caching` on `openai_compatible`, and values that look like secrets.
- **cradle-tools**: the path guard (escapes, symlinks, ignored files); the command runner
  refusing anything not in `[commands]`; `git_diff` being the only git operation.
- **cradle-backends**: request and response conversion for each adapter, using recorded
  fixtures (no network). Parsing the Claude Code `stream-json` and Codex JSONL formats from
  fixtures.

## Integration tests (`tests/`)

- Running the full agent loop against a mock `ModelBackend` that replays scripted tool calls.
- Tests against real providers or CLIs are behind `--ignored` or a `CRADLE_LIVE_TESTS=1`
  env var, and never run by default.

## How to run

See `BUILD.md`: `cargo test --workspace`.

# Tests

Run everything with `cargo test --workspace` (see `BUILD.md`). No test needs network
access, API keys or an installed CLI.

## Unit tests (`#[cfg(test)]` in each crate)

- **cradle-config**: detecting `.agents/` vs `agents.toml` (and the clash); loading this
  repository's own `.agents/`; both forms; mode tables, `[extra]` forms, mode commands
  and path resolution (escapes rejected); skills and front matter; `config.toml` round trip;
  `soul.toml` round trip, partial files and bad values, project `soul.toml` / `[soul]`;
  `theme.toml` parsing (all value forms, per-entry warnings, invalid TOML, overlay).
- **cradle-provider-spec**: validation errors (duplicate `id`, unknown `kind`, bad URL,
  `default_model` not in `models`, fields not allowed for a kind, `prompt_caching` on
  `openai_compatible`, values that look like secrets); runtime overlay merge by `id`.
- **cradle-backends**: the generated provider table matches `providers.toml`; overlay;
  Anthropic and OpenAI request/response conversion; Claude Code `stream-json` and Codex
  JSONL parsing from inline fixtures; live model list handling.
- **cradle-tools**: path guard (escapes, symlinks, `.git/`, ignored and secret files,
  protected files); file tools; command splitting, failure and timeout; tool call
  parsing (including `ask_user` question kinds and option limits); invisible-character
  hygiene; tool variables (`save_as` summaries, name checks, size limits, `var_grep`
  context and separators, `var_replace`, `var_lines` order, output cap).
- **cradle-mcp**: the server over a real local socket: initialize, list and call (routed
  to the channel); refusal without the token or with an `Origin`; unknown tools.
- **cradle-core**: trust snapshot hash stability, diff and store round trip (project
  preferences in the snapshot, stored preference choices); preference resolution; mode
  resolution and clashes; reviewer verdict parsing (missing or garbled means unsure);
  prompt assembly (including each preference band); session log write, list and resume;
  `ask_user` answer formatting.
- **cradle** (bin): theme presets (complete and valid, the dark preset matches the
  original colours and is the `--example` output, roles cascade, palette names resolve,
  bad values keep the preset, optional background); slash command suggestions, model
  picker, markdown rendering, text wrapping, tool cards, and TUI behaviour driven by key
  events (Tab/Shift+Tab mode cycling, input history, sidebar, approvals with requested
  changes, grouped tool calls, multi-question stepping and dismissal, `/theme reload`,
  `/preferences` stepping and saving); preference answers and typed answers.

## Integration tests

`crates/cradle-core/tests/agent_loop.rs` (Unix only) runs the full agent loop against a
scripted fake Anthropic SSE server on 127.0.0.1. It checks that the harness enforces the
rules whatever the model asks for:

- plan mode refuses writes; build mode asks before writing (and "allow all" works)
- declined commands don't run; approved ones do
- the reviewer subagent approves safe commands, and is skipped after protected files change
- mode commands: reviewed once, blocked everywhere if rejected, asked about under
  `/review user`, the only commands a read-only mode may run, auto-approval suspended
  after protected changes
- config reload: a changed config is shown as a diff (decline keeps the old one), an
  approved change applies before the message, an unchanged one is not asked about
- `read_files` returns every file and reports per-file failures
- `save_as` returns a summary instead of the result; `var_*` tools filter saved results
  in plan mode; unknown variables are errors
- preferences are announced once and again after a change; project preferences apply
  only when picked and the choice is remembered; `auto_verify` runs build/test once after
  edits, and reports a failed build to the model

## Rules for new tests

- Never hit the network or real CLIs by default. Use local fake servers and fixtures.
- Tests against real providers or CLIs must be behind `#[ignore]` or a
  `CRADLE_LIVE_TESTS=1` env var. None exist yet.
- Don't touch the OS keychain: use `Auth::None` providers (as `agent_loop.rs` does), or
  set `CRADLE_NO_KEYCHAIN=1`.

# Architecture

## Layout (Cargo workspace)

```
cradle/
  Cargo.toml                 # [workspace] + [workspace.package] + [workspace.dependencies]
  providers.toml             # compile-time provider list (no secrets)
  crates/
    cradle/                  # bin: clap entry, setup, ratatui TUI
    cradle-core/             # agent loop, modes, prompt, trust, sessions, command reviewer
    cradle-config/           # .agents/ + agents.toml loading, modes tables, skills, user config, paths
    cradle-tools/            # path guard, file tools, command runner, tool specs, hygiene
    cradle-provider-spec/    # ProviderSpec types, validation, merge, codegen (light deps)
    cradle-backends/         # build.rs + providers!(); anthropic / openai / cli::{claude, codex}
    cradle-mcp/              # minimal MCP server exposing Cradle's tools to CLI backends
```

Dependency direction (arrows = "depends on"):

```
cradle ─► cradle-core ─► cradle-config
      │              ├► cradle-tools
      │              ├► cradle-backends ─► cradle-provider-spec
      │              └► cradle-mcp
      └► (also cradle-config, cradle-tools, cradle-backends, cradle-provider-spec directly)
cradle-backends (build.rs) ─► cradle-provider-spec   [build-dependency]
```

`cradle-tools` and `cradle-mcp` depend on no other Cradle crate.

## Main modules

- **cradle** (bin): `main.rs` (clap: run, `providers`, `config`), `setup.rs` (trust and
  clash prompts on stdin, session, backend choice), `inspect.rs`, `tui/` (`mod.rs` app
  state, `view.rs`, `commands.rs` slash commands, `onboard.rs` setup screen, `picker.rs`
  model picker, `markdown.rs`, `tools.rs` tool cards, `text.rs` input).
- **cradle-core**: `agent.rs` (the loop, run on its own task, talking to the UI over
  channels; enforces mode access, approvals and trust), `modes.rs` (`ModeSet`: built-in, global
  and project modes), `prompt.rs`, `review.rs` (reviewer subagent), `session.rs`, `trust.rs`.
  Built-in mode prompts are in `modes/` and `prompts/`.
- **cradle-config**: `directory.rs` and `agents_toml.rs` (the two forms), `modes.rs`,
  `skills.rs`, `user.rs` (`config.toml`), `paths.rs`.
- **cradle-tools**: `guard.rs` (`PathGuard`), `fs.rs`, `command.rs`, `specs.rs` (tool
  definitions and call parsing), `hygiene.rs`.
- **cradle-backends**: `anthropic.rs`, `openai.rs`, `cli/` (Claude Code, Codex),
  `message.rs` (canonical message types), `credentials.rs`, `models.rs` (live model lists).

## Key abstractions

- **ProjectConfig**: a single in-memory model built from either `.agents/` or `agents.toml`.
  Nothing else in Cradle cares which format it came from.
- **Backend** (`cradle-backends`): one type over four kinds. Both paths feed the same
  `AgentEvent` stream to the TUI.
  - HTTP (Anthropic, OpenAI-compatible): Cradle runs the agent loop and calls the tools
    itself. Anthropic's message format is the canonical internal format.
  - CLI (Claude Code, Codex): the CLI runs the loop with its built-in tools turned off. The
    agent starts a `cradle-mcp` server (127.0.0.1, random bearer token, Host/Origin checks).
    Each tool call arrives over a channel and goes through the same checks and approvals.
- **Tools**: the only way the agent can affect the world. Every path goes through
  `PathGuard`. Policy (which mode may use which tool, approvals) lives in `cradle-core`.
- **Prompt**: base rules + always-loaded `.agents/` docs + a list of on-demand docs and
  skills. It is built once per session and rebuilt only when project config is reloaded, so
  the prompt cache stays warm. Mode changes are sent as mid-conversation system messages.

## Data locations

| What | Where |
|---|---|
| Default provider and model | `~/.config/cradle/config.toml` |
| Global modes | `~/.config/cradle/modes/<name>.md` |
| Runtime providers overlay | `~/.config/cradle/providers.toml` |
| Trust hashes, source and clash choices | `~/.config/cradle/trust.toml` |
| Approved config snapshots | `~/.config/cradle/trust/<hash>.txt` |
| Sessions (JSONL) | `~/.local/share/cradle/sessions/<project-hash>/` |
| API keys | Env var first, then OS keychain (service `cradle`) |

Overrides: `CRADLE_CONFIG_DIR`, `CRADLE_DATA_DIR`, then `XDG_CONFIG_HOME` /
`XDG_DATA_HOME`. `CRADLE_NO_KEYCHAIN=1` skips the keychain.

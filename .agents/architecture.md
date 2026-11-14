# Architecture

## Current state

A single binary crate: `Cargo.toml` plus `src/main.rs` (a hello-world stub). Dependencies
declared: `clap`, `ratatui`.

## Target layout (Cargo workspace)

```
cradle/
  Cargo.toml                 # [workspace] + [workspace.dependencies]
  providers.toml             # compile-time provider list (no secrets)
  .cargo/config.toml         # optional env (e.g. workspace dir)
  crates/
    cradle/                  # bin: clap entry + ratatui TUI
    cradle-core/             # agent loop, AgentEvent, sessions, prompt assembly
    cradle-config/           # .agents/ + agents.toml loading, modes, soul, trust
    cradle-tools/            # tool registry, path guard, command runner, git_diff
    cradle-provider-spec/    # ProviderSpec serde types + validation (light deps)
    cradle-backends/         # build.rs + providers!() + anthropic / openai_compat / claude_code / codex
    cradle-mcp/              # rmcp server exposing cradle-tools to CLI backends
```

Dependency direction (arrows = "depends on"):

```
cradle ─► cradle-core ─► cradle-config
                      ├► cradle-tools ─► cradle-config
                      └► cradle-backends ─► cradle-provider-spec
                                         └► cradle-mcp ─► cradle-tools
cradle-backends (build.rs) ─► cradle-provider-spec   [build-dependency]
```

## Key abstractions

- **ProjectConfig**: a single in-memory model built from either `.agents/` or `agents.toml`.
  Nothing else in Cradle cares which format it came from.
- **Backends** come in two kinds. Both emit the same `AgentEvent` stream to the TUI.
  - `ModelBackend` (Anthropic, OpenAI-compatible): Cradle runs the agent loop and calls the
    tools itself. Anthropic's message format is the canonical internal format.
  - `AgentBackend` (Claude Code, Codex): the external CLI runs the loop. Cradle spawns it with
    its built-in tools turned off and hands it Cradle's tools through an in-process MCP
    server.
- **Tool registry**: the only way the agent can affect the world. Every tool goes through
  the path guard (confined to the project root, follows `.gitignore`, resolves symlinks).
- **Prompt assembly**: soul + active mode(s) + always-loaded `.agents/` context. It's built
  once per turn and passed to whichever backend is active.

## Data locations

| What | Where |
|---|---|
| Global config, soul, global modes, runtime providers | `~/.config/cradle/` |
| Trust approvals and clash choices | `~/.config/cradle/trust.toml` |
| Sessions (JSONL) | `~/.local/share/cradle/sessions/<project-hash>/` |
| API keys | OS keychain; env vars take precedence |

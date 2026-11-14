# Features

Overview of the implemented features in `features/`.

None yet. All planned work lives in `roadmap/`:

| Roadmap item | Summary |
|---|---|
| [WORKSPACE](roadmap/FEATURE_WORKSPACE.md) | Turn the single crate into the Cargo workspace |
| [CONFIG_LOADING](roadmap/FEATURE_CONFIG_LOADING.md) | Load `.agents/` or `agents.toml` into one model; warn on clash |
| [TRUST](roadmap/FEATURE_TRUST.md) | Approve project config by hash; re-prompt when it changes |
| [MODES](roadmap/FEATURE_MODES.md) | Plan/build/custom modes, `clash_handler`, mode switching |
| [SOUL](roadmap/FEATURE_SOUL.md) | Global behaviour preferences; project override with approval |
| [TOOLS](roadmap/FEATURE_TOOLS.md) | Typed tools, path guard, `[commands]` runner, no raw shell |
| [DOC_PROPOSALS](roadmap/FEATURE_DOC_PROPOSALS.md) | Agent proposes `.agents/` edits as diffs for approval |
| [PROVIDERS_TOML](roadmap/FEATURE_PROVIDERS_TOML.md) | Compile-time `providers.toml` + runtime merge |
| [API_BACKENDS](roadmap/FEATURE_API_BACKENDS.md) | Anthropic + OpenAI-compatible adapters, keychain keys |
| [CLI_BACKENDS](roadmap/FEATURE_CLI_BACKENDS.md) | Claude Code + Codex via an MCP tool server |
| [SESSIONS](roadmap/FEATURE_SESSIONS.md) | JSONL transcripts, `/resume` |
| [TUI](roadmap/FEATURE_TUI.md) | ratatui interface, approvals, diff views |
| [ACP_AGENT](roadmap/FEATURE_ACP_AGENT.md) | Later: run Cradle inside ACP editors |

# Features

Implemented features, with details in `features/`.

| Feature | Summary |
|---|---|
| [WORKSPACE](features/FEATURE_WORKSPACE.md) | Seven-crate Cargo workspace with shared dependencies |
| [CONFIG_LOADING](features/FEATURE_CONFIG_LOADING.md) | `.agents/` or `agents.toml` into one `ProjectConfig`; always-loaded vs on-demand docs |
| [TRUST](features/FEATURE_TRUST.md) | Approve project config by snapshot hash; diff and re-ask when it changes |
| [MODES](features/FEATURE_MODES.md) | Plan/build/scaffold + extra modes, read/full access, `clash_handler`, mode commands, reviewer subagent |
| [SCAFFOLD_MODE](features/FEATURE_SCAFFOLD_MODE.md) | Built-in mode that sets up `.agents/`, the only one allowed to write project config |
| [SKILLS](features/FEATURE_SKILLS.md) | Task instructions in `.agents/skills/`, listed in the prompt, run with `/skill` |
| [TOOLS](features/FEATURE_TOOLS.md) | Typed file tools, path guard, protected files, `[commands]` runner, no raw shell |
| [CONTENT_HYGIENE](features/FEATURE_CONTENT_HYGIENE.md) | Invisible characters shown as `<U+XXXX>` in everything the model reads |
| [PROVIDERS_TOML](features/FEATURE_PROVIDERS_TOML.md) | Compile-time `providers.toml` + runtime overlay merged by `id` |
| [API_BACKENDS](features/FEATURE_API_BACKENDS.md) | Anthropic + OpenAI-compatible adapters; env var or keychain keys |
| [CLI_BACKENDS](features/FEATURE_CLI_BACKENDS.md) | Claude Code + Codex with built-in tools off, Cradle's tools over a local MCP server |
| [SESSIONS](features/FEATURE_SESSIONS.md) | JSONL transcripts, `--continue`, `/session` |
| [TUI](features/FEATURE_TUI.md) | ratatui interface: setup screen, approvals, diffs, sidebar, slash commands |
| [THEME](features/FEATURE_THEME.md) | Global `theme.toml`: dark/light presets, palette, roles, per-element styles, `/theme reload` |

## Roadmap

| Item | Summary |
|---|---|
| [SOUL](roadmap/FEATURE_SOUL.md) | Global behaviour preferences; project override with approval |
| [DOC_PROPOSALS](roadmap/FEATURE_DOC_PROPOSALS.md) | Batched `.agents/` proposals (approved per-write edits already exist) |
| [SANDBOX](roadmap/FEATURE_SANDBOX.md) | Sandbox for `[commands]` |
| [ACP_AGENT](roadmap/FEATURE_ACP_AGENT.md) | Later: run Cradle inside ACP editors |

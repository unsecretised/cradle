# Dependencies

Versions are pinned once in the root `[workspace.dependencies]`. Crates use
`dep = { workspace = true }`.

## Declared now

| Crate | Purpose | Docs |
|---|---|---|
| clap 4.6 | CLI argument parsing | https://docs.rs/clap · https://crates.io/crates/clap |
| ratatui 0.30 | TUI rendering | https://docs.rs/ratatui · https://ratatui.rs |

## Planned

| Crate | Purpose | Docs |
|---|---|---|
| serde | (De)serialization | https://serde.rs · https://docs.rs/serde |
| toml | Parse `.agents` TOML, `providers.toml` | https://docs.rs/toml |
| toml_edit | Only if Cradle rewrites config and must keep comments | https://docs.rs/toml_edit |
| serde_json | API payloads, JSONL sessions, CLI stream parsing | https://docs.rs/serde_json |
| tokio | Async runtime, processes | https://docs.rs/tokio · https://tokio.rs |
| reqwest (rustls, stream) | HTTP to providers | https://docs.rs/reqwest |
| eventsource-stream | SSE parsing for streaming responses | https://docs.rs/eventsource-stream |
| futures | Stream utilities | https://docs.rs/futures |
| schemars | JSON Schema for tool definitions | https://docs.rs/schemars |
| thiserror / anyhow | Errors (libs / bin) | https://docs.rs/thiserror · https://docs.rs/anyhow |
| tracing | Logging | https://docs.rs/tracing |
| crossterm | Terminal backend for ratatui | https://docs.rs/crossterm |
| tui-textarea | Input box | https://docs.rs/tui-textarea |
| pulldown-cmark | Render markdown in TUI | https://docs.rs/pulldown-cmark |
| ignore, globset | File walking that respects `.gitignore` | https://docs.rs/ignore · https://docs.rs/globset |
| similar | Diffs for edit / doc-proposal views | https://docs.rs/similar |
| dunce | Path canonicalisation (cross-platform) | https://docs.rs/dunce |
| directories | Config / data dir locations | https://docs.rs/directories |
| keyring | API keys in OS keychain | https://docs.rs/keyring |
| sha2 | Trust hashes | https://docs.rs/sha2 |
| semver | CLI backend version ranges | https://docs.rs/semver |
| url | Validate `base_url` | https://docs.rs/url |
| rmcp | MCP server for CLI backends | https://docs.rs/rmcp · https://github.com/modelcontextprotocol/rust-sdk |
| agent-client-protocol | Later: expose Cradle as an ACP agent | https://docs.rs/agent-client-protocol · https://agentclientprotocol.com |

## External APIs and tools

- Anthropic Messages API: https://docs.claude.com/en/api/messages
- OpenAI Chat Completions (and compatibles): https://platform.openai.com/docs/api-reference/chat
- Claude Code CLI: `claude --help`, https://docs.claude.com/en/docs/claude-code
- Codex CLI: `codex exec --help`, https://github.com/openai/codex (flags still unverified)

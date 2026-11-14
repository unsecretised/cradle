# Dependencies

Versions are pinned once in the root `[workspace.dependencies]`. Crates use
`dep.workspace = true`.

## In use

| Crate | Used by | Purpose | Docs |
|---|---|---|---|
| clap | cradle | CLI argument parsing | https://docs.rs/clap |
| ratatui | cradle | TUI rendering | https://docs.rs/ratatui · https://ratatui.rs |
| crossterm (event-stream) | cradle | Terminal backend and input events | https://docs.rs/crossterm |
| pulldown-cmark | cradle | Render model markdown in the TUI | https://docs.rs/pulldown-cmark |
| unicode-width | cradle | Text wrapping and layout | https://docs.rs/unicode-width |
| futures-util | cradle, backends | Stream utilities (SSE, terminal events) | https://docs.rs/futures-util |
| tokio | most crates | Async runtime, processes, MCP sockets | https://docs.rs/tokio · https://tokio.rs |
| serde | most crates | (De)serialization | https://serde.rs |
| serde_json | core, tools, backends, mcp | API payloads, JSONL sessions, CLI streams, MCP | https://docs.rs/serde_json |
| toml | config, core, provider-spec | `.agents` TOML, `providers.toml`, `trust.toml` | https://docs.rs/toml |
| thiserror | all | Error types (every crate, including the bin) | https://docs.rs/thiserror |
| reqwest (json, stream) | backends | HTTP to providers | https://docs.rs/reqwest |
| keyring | backends | API keys in the OS keychain | https://docs.rs/keyring |
| semver | backends, provider-spec | CLI backend version ranges | https://docs.rs/semver |
| url | provider-spec | Validate `base_url` | https://docs.rs/url |
| ignore | tools | `.gitignore` matching in the path guard | https://docs.rs/ignore |
| globset | tools | Protected and secret file patterns, `glob` arguments | https://docs.rs/globset |
| regex | tools | `search` tool | https://docs.rs/regex |
| shlex | tools | Split `[commands]` lines without a shell | https://docs.rs/shlex |
| similar | tools, core | Diffs for edits and trust prompts | https://docs.rs/similar |
| sha2 | core | Trust hashes, session directory names | https://docs.rs/sha2 |
| hex | core, mcp | Hash and token encoding | https://docs.rs/hex |
| getrandom | mcp | MCP bearer token | https://docs.rs/getrandom |
| tempfile | backends (+ dev) | Private MCP config file for Claude Code; test dirs | https://docs.rs/tempfile |

## Later

| Crate | Purpose | Docs |
|---|---|---|
| agent-client-protocol | Expose Cradle as an ACP agent | https://docs.rs/agent-client-protocol · https://agentclientprotocol.com |

## External APIs and tools

- Anthropic Messages API: https://docs.claude.com/en/api/messages
- OpenAI Chat Completions (and compatibles): https://platform.openai.com/docs/api-reference/chat
- MCP (streamable HTTP): https://modelcontextprotocol.io
- Claude Code CLI: `claude --help`, https://docs.claude.com/en/docs/claude-code
  (developed against 2.1.311)
- Codex CLI: `codex exec --help`, https://github.com/openai/codex (checked against 0.169.0)

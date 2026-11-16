# CLI backends

Claude Code and Codex run the agent loop with the user's own subscription login. Their
built-in tools are off, so their only tools are Cradle's, served over MCP. Every call
still goes through Cradle's mode checks, path guard and approvals.

## MCP server (`cradle-mcp`)

- A small hand-written MCP server over streamable HTTP (JSON responses), not `rmcp`. It
  is started by the agent loop.
- It listens on 127.0.0.1 on a random port, requires a random 32-byte bearer token, and
  refuses foreign `Host` headers and any `Origin` (DNS-rebinding protection).
- It runs no tools itself: each `tools/call` goes to the agent loop over a channel.
  Tools that aren't read-only are advertised as non-destructive, because Cradle does the
  approving.
- Tool timeout is 24 h so approvals never time out under the user.

## Claude Code (`cli/claude.rs`, developed against 2.1.311)

- `claude -p` with stream-json in and out, `--tools ""` (no built-in tools),
  `--strict-mcp-config --mcp-config <temp file>`, `--allowedTools mcp__cradle`,
  `--setting-sources user` (skips project `.claude/` hooks), `--disable-slash-commands`,
  `--system-prompt`.
- The token is in a private temp file, not on the command line.
- `ANTHROPIC_API_KEY` is removed so the subscription is used.
- Later turns use `--resume <session id>`.

## Codex (`cli/codex.rs`, checked against codex-cli 0.169.0)

- `codex exec --json`, with `--ignore-user-config`. Cradle's server is added with `-c
  mcp_servers.cradle.*` (token via `CRADLE_MCP_TOKEN`, `default_tools_approval_mode =
  "approve"`).
- Disabled features: `shell_tool`, `unified_exec`, `unified_exec_tty`, `computer_use`,
  browsers, `apps`, `multi_agent`, `image_generation`.
- `sandbox_mode = "read-only"` and `approval_policy = "never"`: the OS refuses any write
  Codex attempts on its own.
- There's no system prompt flag, so the first turn carries the instructions. Later turns
  resume by session id.
- `OPENAI_API_KEY` is removed so the ChatGPT login is used.

## Behaviour

- `<binary> --version` is checked against the provider's `version` range. If the CLI is
  missing or outside the range, that provider can't be used and the setup screen explains
  why. This is stricter than the original "warn and continue".
- The CLI's session id is saved in the transcript and resumed across runs (see
  `FEATURE_SESSIONS.md`).
- One-off requests without tools (the reviewer) run in a fresh, unsaved CLI session
  (`--no-session-persistence` / `--ephemeral`).
- Fixture-based parser tests cover each CLI's output format.

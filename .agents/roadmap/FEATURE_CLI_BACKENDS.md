# CLI backends (AgentBackend)

Use the Claude Code and Codex CLIs as backends so people can use their existing
subscriptions. The CLI runs the loop, and Cradle provides the tools.

## Claude Code (flags verified against the installed CLI)

```
claude -p \
  --input-format stream-json --output-format stream-json \
  --tools "" \
  --strict-mcp-config --mcp-config <cradle-mcp.json> \
  --allowedTools "mcp__cradle__*" \
  --system-prompt "<assembled prompt>"
```

`--tools ""` turns off every built-in tool, so the rules are enforced the same way as on the
API backends.

## Codex (UNVERIFIED: Codex is not installed on the dev machine yet)

From memory:
- `codex exec --json` streams JSONL events.
- MCP servers can be added for one run with `-c mcp_servers.cradle...`.
- `--sandbox read-only` blocks writes at the operating-system level.

If its built-in shell can't be turned off, the fallback is a read-only sandbox with all
writes going through Cradle's tools. That's weaker: the shell could still read ignored files
and run read-only commands.

## Decisions

- `cradle-mcp` starts an in-process `rmcp` server for each session, exposing the
  `cradle-tools` registry.
- Each CLI's events are translated into `AgentEvent`.
- At startup, `<binary> --version` is checked against the `version` range in
  `providers.toml`. If it's outside the range, Cradle warns and lets the user continue.
- Fixture-based parser tests for each CLI's output format.

## Open

- Codex: whether the shell can be turned off, its event format, and the version range.

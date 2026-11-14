# ACP agent (later)

Let editors such as Zed host Cradle as an agent over the Agent Client Protocol.

## Decisions

- Not in v1. It comes after the agent loop and tools are stable.
- Uses the `agent-client-protocol` crate. Cradle's loop, tools and rules stay the same, and
  ACP just becomes another front end alongside the TUI.
- Cradle's permission prompts map to ACP `session/request_permission`.

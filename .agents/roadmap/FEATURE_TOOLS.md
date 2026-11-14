# Tools

The agent's only way to act. There is no raw shell.

## Tools

`read`, `write`, `edit`, `search` (content), `glob`, `git_diff`, `run_command`,
`read_agents_doc` (on-demand `.agents/` files), `propose_agents_edit` (see DOC_PROPOSALS).

## Decisions

- **Path guard:** every path is canonicalised and must be inside the project root. Files
  ignored by `.gitignore` are hidden. Writes to `RULES.md`, `modes.toml` and `modes/` are
  always refused, and other `.agents/` writes go through proposals.
- **`run_command`** takes a command *name* from `[commands]`, never a raw string.
- **`git_diff`** is the only git operation.
- Tool schemas are generated with `schemars`, and the same registry serves the API backends
  directly and the CLI backends over MCP.

## Open

- Whether `[commands]` entries take arguments (e.g. `test = "cargo test {filter}"`). If they
  do, every argument needs validating (no shell metacharacters, allowlisted pattern).
- Command timeouts and output size limits.

# Tools

The agent's only way to act. There is no raw shell. Tools live in `cradle-tools`, and
policy (which mode gets which tool, approvals) lives in `cradle-core/src/agent.rs`.

## Tools

| Tool | Access | Notes |
|---|---|---|
| `read_file` | read | Numbered lines, `offset`/`limit` (default 2000 lines, max 2 MiB file) |
| `read_files` | read | Up to 10 files, 2000 lines each; per-file errors |
| `list_files` | read | Optional directory and glob; max 1000 entries |
| `search` | read | Regex over contents; max 200 matches |
| `git_diff` | read | `git diff` (optionally `--cached`, a path). The only git operation |
| `ask_user` | read | One or more questions, no limit: `single` or `multi` choice (2–10 options, own answer allowed) or short `text` |
| `write_file` | write | Shows a diff for approval (depending on the mode's `file_edits`) |
| `edit_file` | write | Exact string replace, unique unless `replace_all` |
| `run_command` | write | A command *name* from `[commands]` or the mode's commands, plus a reason |

The same definitions (hand-written JSON schemas in `specs.rs`) serve the HTTP backends
directly and the CLI backends over MCP (as `mcp__cradle__<tool>`).

## Path guard (`guard.rs`)

- Every path is canonicalised and must stay inside the project root (`..`, absolute
  paths and symlinks that leave it are refused). `.git/` is refused entirely.
- Files matched by `.gitignore` / `.git/info/exclude`, and common secret files (`.env*`,
  keys, `.netrc`, ...), are hidden.
- `.agents/RULES.md`, `modes.toml`, `modes/` and `agents.toml` are read-only (except
  in scaffold mode, with approval).
- Other `.agents/` docs are writable only with the user's approval, after seeing a diff.
  This replaced the planned `propose_agents_edit` tool (see
  `roadmap/FEATURE_DOC_PROPOSALS.md`).
- Protected files need approval because they can make later actions run code: build
  scripts, manifests, `.cargo/`, CI config, editor tasks, shell scripts, toolchain files,
  `.gitignore`/`.gitattributes`/`.gitmodules`, `providers.toml`.
- On approval the user can also ask for changes instead. The request goes back to the model.

## Commands (`command.rs`)

- A command line is split with `shlex` and started directly, with no shell. Entries take
  no arguments.
- The child gets an allowlisted environment (`PATH`, `HOME`, locale, `TERM`, `TMPDIR`,
  cargo/rustup vars), so API keys never reach build scripts.
- 600 s timeout and 20,000 characters kept per output stream. The child is killed if the
  call is cancelled.
- This is not a sandbox (see `roadmap/FEATURE_SANDBOX.md`).

## Output

All tool output passes through the hygiene filter (see `FEATURE_CONTENT_HYGIENE.md`).

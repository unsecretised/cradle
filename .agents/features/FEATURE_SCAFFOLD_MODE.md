# Scaffold mode

A built-in, full-access mode that sets up a project's `.agents/` directory.

- Prompt: `crates/cradle-core/prompts/scaffold.md` (front matter `file_edits: ask`). It
  gathers existing agent instructions (`AGENTS.md`, `CLAUDE.md`, `.cursorrules`, README,
  manifests, ...) and sorts them into the `.agents/` files.
- Only in this mode may the agent write `.agents/RULES.md`, `modes.toml` and `modes/`.
  Every such write needs the user's approval, and the project must be trusted again
  afterwards. `agents.toml` stays read-only.
- Projects and global config cannot define or replace a mode called `scaffold`. Mode
  commands never apply to it.

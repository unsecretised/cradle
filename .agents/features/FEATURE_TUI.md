# TUI

The `cradle` bin's interface (`crates/cradle/src/tui/`), built with ratatui + crossterm.
Model output is rendered as markdown with pulldown-cmark.

## Screens and views

- **Setup screen** (`onboard.rs`): choose a provider, enter an API key if needed (saved to
  the keychain), choose a model. Shown on first run, when the saved provider is unusable,
  and with `--setup`. The choice is saved to `~/.config/cradle/config.toml`.
- **Conversation**: messages, and tool call cards (`tools.rs`). Consecutive reads, lists
  and searches are grouped and can be expanded (Ctrl+O).
- **Approvals**: diffs for writes and config changes, command approvals, and `ask_user`
  questions. The user can approve, decline, or ask for changes.
- **Sidebar**: changed files and usage. Hidden on narrow terminals, toggled with `/sidebar`.
- **Header**: provider/model and the current mode.
- A chat-only provider (`tools = false`) is announced in the conversation when it becomes
  active ("chat only, no file access"), and marked `[chat only]` by `cradle providers`.
- Trust and `.agents/`-vs-`agents.toml` prompts at startup are asked on plain stdin before
  the TUI starts.

## Slash commands (`commands.rs`)

`/help`, `/clear`, `/mode [name]`, `/plan`, `/build`, `/model [provider/model]` (also
switches provider; model picker in `picker.rs`), `/review user|subagent`, `/diff`,
`/session`, `/sidebar`, `/skill <name> [request]`, `/quit`. Aliases: `/models`,
`/sessions`, `/resume`, `/skills`, `/exit`. Suggestions appear while typing, and Tab
completes them.

## Keys

Enter sends. Shift+Enter (also Alt+Enter, Ctrl+J) adds a new line. Tab / Shift+Tab cycle
modes. Esc cancels. Mouse wheel / PgUp scroll. Up/Down walk the input history.

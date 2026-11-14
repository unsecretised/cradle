# TUI

## Decisions

- Built with ratatui + crossterm. Input via tui-textarea. Model output is rendered as
  markdown.
- Views: the conversation, tool call cards, a diff view (edits and doc proposals), trust
  prompts, the config clash prompt and the mode indicator.
- Slash commands: `/mode`, `/resume`, `/provider`, `/model`, `/diff` (runs `git_diff`).
- Shift+Tab cycles modes.
- Clearly shows when the provider is chat-only (`tools = false`).

# Theme

User-level TUI theme in `~/.config/cradle/theme.toml`. Global only: projects cannot set it.

## Format

- `preset = "dark" | "light"` (default `dark`, the original colours).
- `[palette]`: the user's own colour names. Colours are names (`lightblue`), hex
  (`"#61afef"`) or 256-colour indexes (`244`).
- `[roles]`: `accent`, `success`, `warning`, `error`, `muted`, `on_accent`. Elements refer
  to roles, so changing a role recolours everything that uses it.
- Element sections, Helix-style dotted keys: `[ui]`, `[tool]`, `[diff]`, `[review]`,
  `[markdown]`, `[setup]`. A value is a colour (foreground) or `{ fg, bg, modifiers }`;
  modifiers: bold, dim, italic, underlined, reversed, crossed_out. Setting an element
  replaces the preset's style for it. Quoted (`"mode.full"`) and unquoted (`mode.full`)
  dotted keys are the same element.
- `[ui] background`: optional whole-screen style; unset means the terminal's own colours.

Name lookup: palette, then roles, then colour. Layers: dark preset → light preset (if
chosen) → user file.

## Behaviour

- Missing file: the preset. Bad entries are skipped with a warning (shown as conversation
  notices) and the preset's value stays; unparseable TOML gives the preset and one warning.
- Loaded at startup, before the setup screen. `/theme` shows the source, `/theme reload`
  re-reads the file.
- `cradle theme` checks the file; `cradle theme --example` prints the dark preset
  (`crates/cradle/src/tui/themes/dark.toml`), which lists and explains every element.
- The light preset uses 256-colour indexes rather than hex, so it works without
  true-colour support.

## Code

- `cradle-config/src/theme.rs`: lenient parse into strings (no ratatui); `paths::theme()`.
- `cradle/src/tui/theme.rs`: presets, layering and resolution into ratatui `Style`s.

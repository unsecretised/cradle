# Modes

## Decisions

- Built-in defaults: plan and build. Global versions live in `~/.config/cradle/modes/`.
- Project modes: `[default]` replaces plan/build, `[extra]` adds new ones (e.g. `debug_mode`).
- `clash_handler`:
  - `override`: use the project mode only.
  - `combine`: global prompt first, then project prompt. The model is told that the project
    wins on contradictions.
  - `ask`: ask the user each time the clash happens. The answer can be remembered per mode.
- Switching: `/mode <name>` and Shift+Tab to cycle.
- Mode files are subject to trust.

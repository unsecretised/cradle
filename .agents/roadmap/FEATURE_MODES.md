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
- Mode commands: `[<name>_mode.commands]` (e.g. `[plan_mode.commands]`) defines commands
  belonging to that mode. Read-only modes may run only their own commands; full-access
  modes may also request `[commands]`. A name may not mean different lines in a mode
  table and `[commands]`.
- Under `/review subagent`, the first message in a mode each session sends its commands
  to a reviewer (no tools, no history), which gives a verdict per command. Safe commands
  skip approval. Anything else (unsafe, unsure, or a reviewer failure) blocks that command
  line everywhere for the session, including in `[commands]`. A change to the line or a
  new session lifts the block. `/review user` turns auto-approval off.
- Auto-approval is suspended while protected files have changed this session.

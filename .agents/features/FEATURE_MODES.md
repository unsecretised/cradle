# Modes

Modes decide what the agent may do. Access is enforced by the harness, not the prompt.

## Where modes come from

- Built in: `plan` (read-only), `build` (full access) and `scaffold` (full access; see
  `FEATURE_SCAFFOLD_MODE.md`). Prompts are in `crates/cradle-core/modes/` and `prompts/`.
- Global: `~/.config/cradle/modes/plan.md` and `build.md` replace the built-in prompts.
- Project (`modes.toml` or `agents.toml`):
  - `[default]` `plan_mode` / `build_mode` replace plan and build.
  - `[extra]` adds modes: `name_mode = "modes/x.md"` (read-only) or
    `name_mode = { path = "modes/x.md", access = "full" }`. `plan_mode` / `build_mode`
    and `scaffold` are refused there.
- Cycling order: plan, build, scaffold, then the extra modes.

## Access and file edits

- `access = "read"`: read tools, `git_diff`, `ask_user`, and only the mode's own commands.
- `access = "full"`: every tool, subject to approvals.
- A mode file may start with front matter `file_edits: deny | ask | allow`. The default
  is `deny` for read-only modes and `ask` for full access. Read-only modes never edit.
  With `allow`, protected files and `.agents/` docs still ask.

## `clash_handler`

When the project and the user's global config both define plan or build:
- `override`: the project mode only.
- `combine`: the global prompt, then the project prompt. The model is told the project
  wins on contradictions. The project's front matter applies.
- `ask` (default): asked once. The answer is stored per project in `trust.toml`.

## Mode commands and the reviewer

- `[<name>_mode.commands]` (e.g. `[build_mode.commands]`) are commands that belong to that
  mode. Only project config sets them. A name may not mean different lines in a mode
  table and `[commands]`.
- `--review user|subagent` / `/review`: who approves commands.
  - `user` (default): the user approves every `run_command`.
  - `subagent`: a reviewer (a fresh model call, no tools, no history) judges each
    `[commands]` run, and anything not judged safe goes to the user. On the first message
    in a mode, the reviewer gives a verdict per mode command. Safe ones then run without
    asking. Anything else (unsafe, unsure, a reviewer failure) blocks that command line
    for the session, everywhere.
- Auto-approval and the reviewer are suspended once a protected file has changed this
  session.

## Switching

`/mode [name]`, `/plan`, `/build`, Tab / Shift+Tab, or `--mode` at startup. The switch is
announced to the model as a system message listing the tools it may use.

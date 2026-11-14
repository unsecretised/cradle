# Global custom modes

Let users define their own extra modes once, for every project. Today only project config
can add modes (`[extra]`). `~/.config/cradle/modes/` only accepts `plan.md` and `build.md`,
which replace the built-in prompts.

## Proposed

- Any other `~/.config/cradle/modes/<name>.md` becomes a global extra mode called `<name>`.
- Settings go in front matter, next to the existing `file_edits`:

  ```markdown
  ---
  access: read          # read (default) | full
  file_edits: ask       # deny | ask | allow (same defaults as project modes)
  cycle: false          # default false: only reachable via /mode <name> or --mode
  ---
  ```

- **Not in the Tab / Shift+Tab cycle by default.** `cycle: true` adds the mode, after the
  project's extra modes. `/mode` with no argument lists every mode, including the ones
  that aren't cycled.
- Global modes are the user's own config, so they don't go through trust (like global
  `plan.md` / `build.md` today).
- `plan`, `build` and `scaffold` keep their current meaning. A global file can't create a
  mode with one of those names.
- If a project `[extra]` mode has the same name, `clash_handler` decides
  (`override` / `combine` / `ask`), as it does for plan/build. The answer is stored per
  project in `trust.toml`.

## Open

- Commands: today only project config can set mode commands (`[<name>_mode.commands]`).
  Should a global mode get its own commands? They would have to be checked against the
  project's `[commands]`, because the project decides what may run there.
- Front matter vs. a `~/.config/cradle/modes.toml` index that mirrors the project format.
- Should `cycle` also be a setting for project `[extra]` modes?

# Soul (Preferences)

User-level behaviour preferences, called "Preferences" in the UI. Code:
`cradle-config/src/soul.rs` (file format), `cradle-core/src/soul.rs` (which values
apply), `cradle-core/src/prompt.rs` (`soul_message`), `cradle/src/tui/preferences.rs`.

## The file

`~/.config/cradle/soul.toml`. Every key is optional; the defaults are shown.

```toml
questions = 5              # 1-10: 1 = ask only when blocked, 10 = check in on every decision
audience = "technical"     # technical | non-technical
detail = "concise"         # concise | detailed: how much the agent explains
auto_verify = false        # run build/test after edits automatically
```

Unknown keys and out-of-range values are errors. `/preferences` writes every key.

## Setting them

- After the setup screen (first run, `--setup`), the preferences questions follow.
- `/preferences` (alias `/soul`) asks them again in the question modal, starting at the
  current values (Enter keeps each). Typed answers are read when clear ("8", "plain",
  "on"); otherwise that value is kept. Saved answers apply from the next message.
- Users with a provider but no `soul.toml` get a notice pointing at `/preferences`.

## What the model sees

A system message at the start of each conversation (like the mode), and again when the
preferences change, so the system prompt and its cache stay unchanged. `questions` maps to
five fixed sentences (1-2, 3-4, 5-6, 7-8, 9-10). `questions`, `audience` and `detail` guide
the model; only `auto_verify` controls the harness.

## Project override

- A project may set any subset in `.agents/soul.toml` or `[soul]` in `agents.toml`. The
  agent cannot write either (`soul.toml` is project config, like `modes.toml`).
- Set values are part of the trust snapshot, so they show in the trust diff. A config
  without preferences keeps its existing hash.
- Where a project value differs from the user's, the user picks "use the project's" or
  "keep mine" before the next message. The answer is stored per project in `trust.toml`
  with the project's value, and asked again if that value changes. Dismissed: the user's
  own values apply and Cradle asks again next message. A project cannot skip this.

## `auto_verify`

After a reply that changed files in a full-access mode, the harness runs `build` and then
`test` from `[commands]` (or the mode's own commands), those that exist, through the usual
approvals and reviewer. Shown as tool cards marked `(auto_verify)`.

- Both pass: a notice, no model call.
- One fails (non-zero exit): later checks are skipped and the output goes to the model as
  a system message so it can fix it. At most one verify round per user message.
- One doesn't run (declined, blocked, cancelled): the check stops, nothing is sent.
- CLI backends: a failure report starts another CLI turn as a harness notice.

# Soul (Preferences)

User-level behaviour preferences, turned into prompt instructions (and some harness
behaviour). Called "Preferences" in the UI.

## Decisions

- Lives in `~/.config/cradle/soul.toml`.
- Set during first-run setup (after provider and model), with `--setup`, and with
  `/preferences` (alias `/soul`). Users with a provider but no `soul.toml` get a notice
  pointing at `/preferences` instead of the setup step.
- Told to the model as a system message at the start of each conversation (like the mode)
  and again when they change, so the system prompt and its cache stay unchanged.

Implemented: see `features/FEATURE_SOUL.md`.
- A project may override it in `.agents/soul.toml` (or `[soul]` in `agents.toml`). The file
  is read-only to the agent and part of the trust snapshot. When a project value differs
  from the user's, the user picks "project" or "mine" once per project (`trust.toml`). A
  project cannot skip that question.

## Shape

```toml
questions = 5              # 1-10: 1 = ask only when blocked, 10 = check in on every decision
audience = "technical"     # technical | non-technical
detail = "concise"         # concise | detailed: how much the agent explains
auto_verify = false        # run build/test after edits automatically
```

Every key is optional; the defaults are shown above. A project file may set any subset.

Fixed options rather than free text, so values can be checked. `questions` maps to five
fixed prompt sentences (1-2, 3-4, 5-6, 7-8, 9-10). `questions`, `audience` and `detail`
guide the model; only `auto_verify` controls the harness.

## `auto_verify`

After a turn that changed files in a full-access mode, the harness runs the `build` and
then `test` commands from `[commands]` (those that exist), through the usual approvals
and reviewer. The results go back to the model as a system message so it can fix
failures. At most one verify round per user message.

## Open

- More options later.

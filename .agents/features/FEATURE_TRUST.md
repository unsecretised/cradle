# Trust

Project config applies only after the user approves it, and again whenever it changes.

- Cradle renders the trusted parts of the config (modes, mode files, `[commands]`, mode
  commands, project preferences, `RULES.md`, skills) as a text snapshot
  (`cradle-core/src/trust.rs`).
- `~/.config/cradle/trust.toml`, keyed by canonical project path, records the snapshot's
  sha256, the `.agents/`-vs-`agents.toml` choice and the remembered `clash_handler = "ask"`
  answer. The approved snapshot itself is in `~/.config/cradle/trust/<hash>.txt`.
- `trust.toml` also stores, per project preference that differs from the user's, the
  project's value and whether the user chose it or kept their own (see `FEATURE_SOUL.md`).
- On startup (plain stdin, before the TUI): a known hash applies silently. Otherwise
  Cradle lists the commands and modes, shows the diff against the last approved snapshot
  (or the full config) and asks. Declined: Cradle runs with no project modes or
  commands, and the prompt says so.
- Config is reloaded before every message. A changed config shows a diff in the TUI.
  Approved: it applies before the message. Declined: the old config stays.

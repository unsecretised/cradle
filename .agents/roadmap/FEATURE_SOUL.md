# Soul

User-level behaviour preferences, turned into prompt instructions (and some harness
behaviour).

## Decisions

- Lives in `~/.config/cradle/soul.toml`.
- A project may override it, with approval (trust) and following `clash_handler`.

## Proposed shape (not final)

```toml
questions = "fewer"        # more | fewer
auto_verify = true         # run build/test after edits automatically
audience = "technical"     # technical | non-technical
```

Fixed options rather than free text, so values can be checked and some can control the
harness directly (`auto_verify` triggers the `[commands]` build/test).

## Open

- The final list of options.

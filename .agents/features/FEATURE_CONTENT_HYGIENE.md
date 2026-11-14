# Content hygiene

Hidden text can carry instructions a reviewer never sees. `cradle-tools/src/hygiene.rs`
rewrites invisible characters as `<U+XXXX>` before they reach the model or the user.

- Covered: control characters (except `\n`, `\r`, `\t`), zero-width and joiner characters,
  bidi controls, variation selectors, BOM, Hangul fillers, private-use areas and Unicode
  tags.
- Applied to tool results, command output (which also has ANSI escapes stripped), the
  agent's stated reason for a command, the always-loaded `.agents/` docs, and skill
  descriptions in the system prompt.
- The system prompt tells the model to treat such text as suspicious and point it out.

Note: `hygiene.rs` still links to `roadmap/FEATURE_CONTENT_HYGIENE.md`. The doc lives here.

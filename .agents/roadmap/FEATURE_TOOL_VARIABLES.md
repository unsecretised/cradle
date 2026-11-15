# Tool variables

Let the agent save a tool result in a named variable, filter it with small text tools, and
use it in later calls, without the full output ever entering the context. This reduces
token use the way a scripting tool would, but needs no script engine or new dependency
(`regex` is already used by `cradle-tools`).

## Proposed

- **`save_as`.** Every tool takes an optional `save_as` (`[A-Za-z_][A-Za-z0-9_]*`, at
  most 32 characters). The full result is stored, and the model gets a short summary
  instead: `saved $name: N lines, X KB (from <call summary>)`, plus the first 5 lines.
  Error results are never saved.
- **Variable tools** (read access, no file system):

  | Tool | Does |
  |---|---|
  | `var_grep {var, pattern, context?, invert?, only_matching?, save_as?}` | Keep lines matching a regex |
  | `var_replace {var, pattern, replacement, save_as?}` | Regex replace on each line (`$1` groups) |
  | `var_lines {var, head?, tail?, from?, to?, unique?, sort?, save_as?}` | Slice, de-duplicate, sort lines |
  | `var_show {var}` | Return the full contents to the model |
  | `var_list {drop?}` | Names, sizes and sources of saved variables; optionally drop some |

  Without `save_as`, a variable tool returns its result to the model (capped like other
  tool output).
- **References** (second step). A JSON value `{"$var": "name"}` inside a call's input is
  replaced before parsing. In a string field it becomes the text; in an array field, one
  item per non-empty line (`read_files.files` items become `{path}`). Every normal check
  (mode access, `PathGuard`, approvals) then runs on the resolved call, and the tool card
  shows the resolved input.
- **Scope.** Variables live in memory for the session, and are cleared on `/session`
  switch, resume and config reload. The summary records which call produced a variable,
  because it can go stale after an edit.
- **Limits.** 2 MiB per variable, 16 MiB in total. Going over either is a tool error, not a
  silent truncation.
- **Hygiene.** Stored values are the same as what the model would have seen
  (`<U+XXXX>` already applied).

## Defaults chosen

- References are allowed only in read tools at first (`write_file`, `edit_file` and
  `run_command` refuse them). Stored values have line numbers and hygiene markers, so
  passing them into a write would corrupt files.
- Five separate `var_*` tools rather than one tool with an `op` field (clearer schemas;
  the definitions are cached).
- Two commits: (1) `save_as` and the `var_*` tools, (2) references.

## Where

- `cradle-tools/src/vars.rs` (new): store, limits, filters, reference resolution (pure,
  unit tested).
- `cradle-tools/src/specs.rs`: `save_as` on every schema, the new tools, summaries and
  activities.
- `cradle-core/src/agent.rs`: resolve references before `parse_call`, save after
  `run_call`, clear on session change.

## Open

- Whether write tools should later accept references to raw (unnumbered, unfiltered)
  values.
- Whether variables should survive a resume (they'd need to be written to the transcript).

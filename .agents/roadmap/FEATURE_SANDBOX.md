# Command sandbox

`[commands]` entries run without a shell and with a scrubbed environment
(`cradle-tools/src/command.rs`), but they are not sandboxed. A build script or test can
still read and write anything the user can.

## Open

- TODO: approach (e.g. macOS `sandbox-exec`, Linux namespaces / Landlock), and what the
  sandbox allows (writes limited to the project and `target/`? network?).
- TODO: how it interacts with the reviewer and mode-command auto-approval.

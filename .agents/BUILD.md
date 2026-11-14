# Build, run, test

Requires Rust 1.88+ (edition 2024; let-chains need 1.88). Set in `[workspace.package]`.

The exact commands an agent may run are in `modes.toml` under `[commands]`. This file
explains them for humans.

| Task | Command | Notes |
|---|---|---|
| Build | `cargo build --workspace` | |
| Run | `cargo run -- <args>` | Runs the `cradle` bin |
| Test | `cargo test --workspace` | See `TESTS.md` |
| Lint | `cargo clippy --workspace --all-targets -- -D warnings` | Must pass with no warnings |
| Format | `cargo fmt --all` | |
| Format check | `cargo fmt --all --check` | |

## CLI

```
cradle [--root DIR] [--provider ID] [--model NAME] [--mode NAME]
       [--review user|subagent] [--setup] [-c|--continue]
cradle providers                     # compiled-in + ~/.config/cradle/providers.toml
cradle config [--root DIR] [--source dir|file]   # show the project config Cradle loads
```

- `--mode` defaults to `plan`, `--review` to `user`.
- `--setup` shows the provider setup screen even if a default is saved.
- `--continue` resumes the most recent conversation in this project.

## Notes

- `cradle-backends/build.rs` regenerates the provider table whenever `providers.toml`
  changes. If the build fails with a provider validation error, the error points at the
  offending entry in `providers.toml`.
- No API keys are needed to build or test.

# Build, run, test

Requires Rust with edition 2024 support (1.85+). Developed on rustc 1.99.

The exact commands an agent may run are in `modes.toml` under `[commands]`. This file
explains them for humans.

| Task | Command | Notes |
|---|---|---|
| Build | `cargo build --workspace` | Works on the current single crate too |
| Run | `cargo run -- <args>` | Once the workspace exists: `cargo run -p cradle -- <args>` |
| Test | `cargo test --workspace` | |
| Lint | `cargo clippy --workspace --all-targets -- -D warnings` | Must pass with no warnings |
| Format check | `cargo fmt --all --check` | |

## Notes

- `cradle-backends/build.rs` regenerates the provider table whenever `providers.toml`
  changes. If the build fails with a provider validation error, the error points at the
  offending entry in `providers.toml`.
- No API keys are needed to build. Tests that hit real providers must be opt-in (see
  `TESTS.md`).

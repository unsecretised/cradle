# Workspace setup

Turn the single crate into the Cargo workspace described in `architecture.md`.

## Decisions

- Root `Cargo.toml` is a workspace only, with `resolver = "3"`, and holds shared
  `[workspace.package]` (edition 2024) and `[workspace.dependencies]`.
- Crates live in `crates/`. The current `src/main.rs` moves to `crates/cradle/src/main.rs`.
- Seven crates: cradle, cradle-core, cradle-config, cradle-tools, cradle-provider-spec,
  cradle-backends, cradle-mcp. Don't split further without a reason.

## Done when

- `cargo build --workspace`, `cargo clippy ... -D warnings` and `cargo test --workspace` pass
  on empty crates.
- `cargo run -p cradle` runs the stub.

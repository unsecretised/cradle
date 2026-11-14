# Workspace

The project is a Cargo workspace (see `architecture.md` for the crate list and dependency
direction).

- Root `Cargo.toml` is a workspace only: `resolver = "3"`, `members = ["crates/*"]`,
  shared `[workspace.package]` (version, edition 2024, `rust-version = "1.88"`) and
  `[workspace.dependencies]` for both internal and external crates.
- Seven crates: cradle, cradle-core, cradle-config, cradle-tools, cradle-provider-spec,
  cradle-backends, cradle-mcp. Don't split further without a reason.

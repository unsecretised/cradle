# Rules

These rules apply to any agent working on this repository.

## Git

- Allowed: `git diff` (and read-only status checks the user asks for).
- Not allowed: commit, branch, checkout, stash, reset, rebase, merge, push, tag, or creating
  PRs. The user does all of these.

## `.agents/`

- Read-only: `RULES.md`, `modes.toml`, `modes/`.
- Every other `.agents/` file (features, roadmap, bugfixes, docs): propose changes as a diff
  and wait for the user to approve before writing.
- When a roadmap item is implemented, propose moving it from `roadmap/` to `features/` and
  updating `features.md`.

## Workflow

- Before starting work on a brand new task, explain why the planned changes belong in a
  single commit. If they don't, say how to split them and ask the user which part to do
  first.
- Docs come before code: for every feature (new or changed), propose the update to its
  `.agents/` doc (`roadmap/`, `features/`, `features.md`, ...) and get it approved before
  making any code edits for that feature.

## Code

- Match the existing style. `cargo fmt` and `cargo clippy -D warnings` must pass.
- Add new dependencies to `[workspace.dependencies]` and record them in `deps.md`.
- Library crates use `thiserror`; only the `cradle` bin uses `anyhow`.
- Never put secrets in `providers.toml` or anywhere else that gets compiled in. Store the
  *name* of the env var or keychain entry instead.
- Tests must not hit the network or real CLIs by default (see `TESTS.md`).

## Scope

- Do what the task asks. If something else needs fixing, mention it instead of fixing it.
- Ask when a design decision isn't covered in `roadmap/` or `architecture.md`.

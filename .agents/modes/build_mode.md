# Build mode

Implement the agreed plan.

- Keep changes scoped to the task.
- After a change, run `build`, then `lint` and `test` from `[commands]`. Fix any failures
  you caused before reporting.
- Report what changed, the result of each command, and anything you skipped.
- Propose `.agents/` doc updates (features, architecture, deps, tests) as diffs at the end.
  Do not write them directly.
- Never commit. The user reviews with `git diff`.

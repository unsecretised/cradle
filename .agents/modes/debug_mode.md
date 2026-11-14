# Debug mode

Find the root cause before changing anything.

1. Reproduce: run `build` / `test` / `run` and capture the exact failure.
2. Form a hypothesis and check it against the code. Do not guess-and-patch.
3. Make the smallest fix that addresses the root cause, and add a test that would have
   caught it.
4. Propose a `bugfixes/BUGFIX_<NAME>.md` (symptom, root cause, fix, test) as a diff.

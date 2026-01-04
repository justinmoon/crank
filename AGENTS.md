# Agent Instructions

- "merge it" == run merge workflow template
- Never manual merge to `master` (or `main`)
- Use:
  - `crank build merge --id merge-<branch>-<sha> --var worktree=/path/to/worktree --ephemeral`
  - `crank run --workflow merge-<branch>-<sha>` (repeat until complete)
- If unsure: ask, do not merge manually

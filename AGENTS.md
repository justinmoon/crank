# Agent Instructions

- Run `./scripts/agent-brief` once at the start of each new agent session in this worktree (not every turn).
- Rerun only if asked, if you switch worktrees, or if the first run failed.
- "land it" == run land workflow template
- Never manual merge to `master` (or `main`)
- Use:
  - `crank build land --id land-<branch>-<sha> --var worktree=/path/to/worktree --ephemeral`
  - `crank run --workflow land-<branch>-<sha>` (loops until waiting/complete; use `--once` for a single step)
- If `pre-merge` or `review` fails, fix the issue and rerun the workflow; `scripts/land/review.sh` prints the review failure details.
- If unsure: ask, do not merge manually

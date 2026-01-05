# Agent Instructions

- "merge it" == run merge workflow template
- Never manual merge to `master` (or `main`)
- Use:
  - `crank build merge --id merge-<branch>-<sha> --var worktree=/path/to/worktree --ephemeral`
  - `crank run --workflow merge-<branch>-<sha>` (loops until waiting/complete; use `--once` for a single step)
- If `pre-merge` or `review` fails, fix the issue and rerun the workflow; `scripts/merge/review.sh` prints the review failure details and can write a log via `CRANK_REVIEW_LOG=/path/to/file`.
- If unsure: ask, do not merge manually

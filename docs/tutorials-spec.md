# Tutorial Browser Spec

## Goal

Provide a post-merge, inbox-style tutorial browser so a developer can quickly
understand what was built for each task/PR without re-reading raw diffs.

Key outcomes:
- A new `crank inbox` command to browse tutorials (like an email inbox).
- Auto-generated tutorials after merge, stored for later viewing.
- A reader that shows: issue -> summary -> (explanation, diff) pairs.
- Diff rendering via `$EDITOR`, ideally embedded in the TUI (portable-pty).

## Non-goals

- Replacing code review or tests.
- Perfect semantic decomposition of changes on day one.
- Mandatory dependency on any specific editor or LLM provider.

## Command Surface

### `crank inbox`
Opens a TUI that lists tutorials for the current repo.

### `crank tutorial generate`
Generate and store a tutorial for a merge commit.

Proposed flags:
- `--worktree <path>`: worktree that produced the merge.
- `--base <branch>`: base branch (default: master).
- `--merge-commit <sha>`: merge commit SHA (if omitted, infer last merge to base).
- `--workflow-id <id>`: optional workflow id for metadata.
- `--output-dir <path>`: override tutorial storage location.

### `crank tutorial show <id>`
Print the tutorial in a plain format (markdown or json) for scripting.

Suggested flags:
- `--format md|json` (default md)
- `--step <n>`: print only one step.

### Open tutorial
Select an entry in `crank inbox` to open the tutorial viewer.

## Storage Layout

Store repo-scoped tutorials under `.crank/tutorials/`.

```
.crank/
  tutorials/
    index.json
    <tutorial-id>/
      tutorial.json
      issue.md
      summary.md
      steps/
        01.md
        01.diff
        02.md
        02.diff
```

`tutorial.json` should be a small indexable blob:
- `id` (ex: `merge-branch-sha`)
- `title` (issue title)
- `issue_ids` (from `.crank/.current`)
- `created_at`
- `merge_commit`
- `base_branch`
- `source_branch`
- `status` (`unread`/`read`)
- `steps` metadata (count, files touched, commit range)

`index.json` is a cache for fast listing; regenerate if missing.

## Tutorial Content Structure

1) Issue
- The original task text from `.crank/<id>.md`.
- Should include title + intent/spec sections if present.

2) Summary
- 5-10 bullets: what was implemented and how it was verified.
- Include tests run (from merge workflow steps if available).

3) Step Walkthrough (pairs)
- Each step is an explanation + diff.
- Default heuristic: one step per commit in `base..merge_commit` order.
- Alternative grouping: one step per file group if commit history is noisy.

Each step is stored as:
- `steps/NN.md`: explanation markdown
- `steps/NN.diff`: unified diff for that step

## Generation Flow

`crank tutorial generate` should:

1) Resolve merge metadata:
   - `worktree` -> `source_branch` + `.crank/.current`.
   - `merge_commit` (from merge step output or infer with `git log`).
2) Load issue content from `.crank/<id>.md`.
3) Collect commits: `git log --reverse <base>..<merge_commit>`.
4) For each commit:
   - Extract patch: `git show <commit>`.
   - Generate explanation (LLM or fallback heuristic).
5) Generate a top-level summary for the full merge range.
6) Write tutorial files + update `index.json`.

LLM usage (optional, recommended):
- Use `opencode run` with a dedicated prompt for summary + per-step explanation.
- Fallback if LLM not available: use commit subject lines + file list.

## TUI UX (Inbox Style)

Inbox list fields:
- Title (issue title)
- Branch or task id
- Date merged
- Read/unread status

Key actions:
- `Enter`: open tutorial
- `r`: mark read/unread
- `/`: filter/search (title, id, branch)
- `q`: quit

Tutorial viewer layout:
- Top: issue summary (markdown rendered as plain text)
- Middle: summary bullets
- Bottom: step list with selection
- Right pane or overlay: explanation markdown
- Diff opens in embedded `$EDITOR` (portable-pty)

Diff rendering behavior:
- Default: open `steps/NN.diff` in `$EDITOR` read-only.
- Fallback: open `git show` in a new tmux pane if not embedded.

## Jump-to-Definition / File Navigation

Support opening source files at the merge commit:
- Create a read-only worktree at `.crank/tutorials/.worktrees/<merge_commit>`.
- Provide an action to open the selected file in `$EDITOR` from that worktree.
- Optional cleanup policy (keep last N worktrees).

## Merge Workflow Integration

Add a post-merge step (after `merge`) once implemented:
- `tutorial` step runs `scripts/merge/tutorial.sh`.
- Script calls `crank tutorial generate` using:
  - `--worktree` (merge workflow var)
  - `--base`
  - `--merge-commit` (from merge output or `git rev-parse` on base)

This step should be best-effort:
- If tutorial generation fails, do not fail the merge.

## Implementation Notes (Phase Plan)

Phase 1: Generate + show
- Implement `crank tutorial generate` + `crank tutorial show`.
- Store `tutorial.json`, summary, and per-commit diffs.

Phase 2: Inbox TUI
- Reuse `src/task/tui` patterns for list/preview behavior.

Phase 3: Embedded editor
- Add `portable-pty` and render `$EDITOR` for diffs.
- Add fallback to tmux split if not available.

Phase 4: Jump-to-definition
- Worktree cache + open file action.

## Open Questions

- Should tutorial storage live under repo `.crank/` or user `~/.crank/`?
- How aggressively should we prune old tutorials/worktrees?
- Should summary include workflow step logs (if present)?

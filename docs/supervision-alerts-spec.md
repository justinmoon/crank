# Supervision + Alerts Spec

## Goals
- Formalize supervised vs unsupervised modes in task frontmatter and CLI.
- Supervised mode is fully manual: show the task picker first, require a selection, run one task, then return to the picker.
- Unsupervised mode auto-claims only unsupervised tasks.
- Replace notification tooling with a tmux modal alert list plus a lightweight macOS menu bar badge.

## Terminology
- `supervised`: task runs with explicit user selection each round.
- `unsupervised`: task is auto-claimed by workers.

## Task Frontmatter
- Replace `autopilot` with `supervision`.
- Allowed values: `supervised`, `unsupervised`.
- No legacy compatibility; migrate existing task files.

Example:
```
supervision: supervised
```

## CLI
- `crank tmux` requires `--mode supervised|unsupervised` (no default).
- `crank worker` requires the same mode (internal; passed from tmux).

## Supervised Worker Flow
1) Show task picker (ranked by priority). No exit without a selection.
2) User selects a task, or uses `n`/`e` to create/edit tasks.
3) Run the task (worktree + agent).
4) On task completion or help request:
   - terminate the agent
   - create an alert
   - return to the task picker

## Unsupervised Worker Flow
- Auto-claim next task.
- Ignore tasks with `supervision: supervised`.
- Unsupervised agents may not call `crank ask-for-help`.

## Alerts
### Storage
- Alerts live in `~/.crank/alerts/` as JSON files.
- Each alert includes: task id, title, alert type, tmux window/pane targets, timestamp.
- Alert count is derived by counting `.json` files in the alerts directory.

### Alert Types
- `completed`: task finished and needs user attention.
- `needs_help`: supervised task asked for help.

### Creation
- On supervised task completion or help request, write an alert.
- Emit a tmux popup that shows the alert list.

### Tmux Modal UI
- New `crank alerts` command renders a TUI list of alerts.
- Opens via `tmux display-popup` from the worker.
- `crank alerts --watch` runs a watcher that auto-pops the alerts modal on new alerts for all attached tmux clients.
- Selection moves to the alert's tmux window/pane and clears the alert.
- Keys: `j/k` (move), `enter` (jump), `d` (dismiss), `q` (quit).

## Swift Menu Bar Badge
- Minimal Swift app that shows the current alert count in the menu bar.
- Counts `.json` files in `~/.crank/alerts`.
- No Xcode GUI tools; build from the CLI using swiftc or swift build.

## Migration
- Update `.crank/*.md` frontmatter:
  - `autopilot: true` -> `supervision: unsupervised`
  - `autopilot: false` -> `supervision: supervised`
- Update templates in code to emit `supervision`.

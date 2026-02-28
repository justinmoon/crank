# Workflow Orchestration Spec

## Goals

- Workflows are pure task graphs; the binary is not project-specific.
- Task and workflow execution share the same command surface.
- Deterministic progression within a workflow (single worker per workflow).
- Reuse the same agent session between workflow steps.

## Non-goals (for now)

- Parallel step execution within a workflow.
- Multiple workers on the same workflow.
- Project-specific merge logic in the binary.

## Concepts

- Task: a markdown file in `.crank/` with frontmatter + body.
- Workflow template: TOML blueprint that expands into tasks.
- Workflow instance: the set of tasks created from a template.

Once instantiated, a workflow is just tasks + dependencies.

## Files

- Templates: `.crank/workflows/*.workflow.toml`
- Tasks: `.crank/<workflow-id>.<step-id>.md`
- Current task marker: `.crank/.current`
- (Optional) Workflow manifest: `.crank/workflows/<workflow-id>.manifest.toml`

## Frontmatter fields

Required for workflow steps:
- `workflow: <workflow-id>`
- `step_id: <step-id>`
- `depends_on` (from template)

No execution-context fields (tmux/session/worktree) live in task files.

## Task body format (command vs agent steps)

Command steps define the command and expected output in markdown:

````md
## Spec
### Run
```bash
<command>
```

### Acceptable Output
- Describe what success looks like.
````

If a task has no `### Run` section, it is an agent-driven step.

## Command surface (unified)

### Build a workflow instance

```
crank build <template> --id <workflow-id> --var key=val
```

Creates tasks + manifest. Canonical replacement for `workflow apply`.
The `workflow` subcommand is removed in favor of `build` + `run`.

### Run work

```
crank run
crank run --workflow <workflow-id>
crank run --workflow <workflow-id> --once
crank run <task-id>
```

- `crank run`: pick the next task using sticky workflow context.
- `crank run --workflow`: force workflow scope and loop until waiting/complete.
- `crank run --workflow --once`: run a single workflow step.
- `crank run <task-id>`: run a specific task.

### Task management (unchanged)

```
crank task create|done|dep|hooks|claim
```

## Deterministic “next” (sticky workflow context)

Invariant: only one worker processes a given workflow at a time.

Selection logic:
1) If `.crank/.current` points to a workflow step:
   - Determine the **next step in workflow order**.
   - If that step is blocked, **wait** (do not pick a different task).
   - If no steps remain, fall back to global task selection.
2) Otherwise, select the next global task by priority + FIFO (current behavior).

Workflow order is defined by the template’s step list. The apply command writes a
manifest file containing ordered step IDs. If the manifest is missing, step order
falls back to lexicographic `step_id`.

## Execution semantics

- If the task body includes a `### Run` section, `crank run` executes the command.
- Non-zero exit status fails the step; output is logged.
- `### Acceptable Output` is informational for humans/agents and is not parsed.
- If there is no `### Run`, `crank run` launches or continues an agent session.

## Agent session continuity across workflow steps

When executing consecutive steps in the same workflow:
- Keep the same agent session alive.
- Send a new prompt that points to the next task file.
- If the session is missing, recreate it and continue.

The worker already runs inside tmux; window reuse is implicit and not managed per step.

## Removal of project-specific merge logic

Remove from the binary:
- `crank merge`
- `merge-step` subcommands
- merge status/attach commands tied to `~/.crank/merges`
- merge-specific approval commands (`approve`, `reject`, `pending`)

Merge and release are **repo-level workflows**:
- Implement steps as repo scripts or `just` targets.
- Templates call those scripts via task body `### Run` commands.

## Merge workflow (repo-level)

Goal: a fully working merge workflow implemented in repo scripts, with no hard-coded merge logic in the binary.

Required steps and dependencies:

1) `preflight`
   - Fetch base branch, ensure clean worktree, ensure branch is ahead of base.
2) `pre-merge` (needs `preflight`)
   - Run project CI (e.g. `just pre-merge`).
3) `review` (needs `preflight`)
   - Run agentic review (e.g. `crank review` or repo script).
   - Output must begin with `PASS` or `FAIL: <reason>`.
4) `conflicts` (needs `pre-merge`, `review`)
   - Check `git merge-tree` against `origin/<base>`.
5) `approval` (needs `conflicts`)
   - Human gate if notifications are enabled.
6) `merge` (needs `approval`)
   - Merge into base in the main worktree and push.

These steps are implemented as repo scripts or `just` targets (e.g. `scripts/merge/*.sh`).
The template only wires dependencies and command invocations.

## Expected behavior (summary)

- Workflows behave like tasks; “next” is deterministic and workflow-aware.
- Agents keep context across workflow steps (same session).
- The binary is generic; project-specific behavior lives in templates/scripts.

# Workflows (Agent Overview)

Workflows are just graphs of tasks. A workflow template defines steps and dependencies, and `crank` expands it into `.crank/*.md` tasks that the runner executes. This lets agents follow a clear DAG without digging into implementation details.

## Mental model

- Template = blueprint (steps + needs)
- Instance = concrete run (tasks + status)
- Execution = run steps in template order, one at a time

A step is a task. Dependencies are `depends_on` entries with `type: blocks`.

## Where templates live

- Repo templates: `.crank/workflows/*.workflow.toml`
- User templates: `~/.crank/workflows/*.workflow.toml`

## Template format (TOML)

Each step is a node. `needs` are edges. `run` is optional (missing means human gate).

```toml
workflow = "merge"
version = 1

[vars]
base = { default = "master" }

[[steps]]
id = "preflight"
title = "Preflight checks"
run = "scripts/merge/preflight.sh --base {{base}}"

[[steps]]
id = "review"
title = "Run review"
run = "scripts/merge/review.sh"
needs = ["preflight"]
```

## Commands (what an agent needs)

- Build a workflow: `crank build <template> --id <workflow-id> --var key=val`
- Run next step: `crank run`
- Run a workflow (loops until waiting/complete): `crank run --workflow <workflow-id>`
- Run a single workflow step: `crank run --workflow <workflow-id> --once`

Apply creates tasks in `.crank/` like:
- `workflow: <workflow-id>`
- `step_id: <step-id>`
- `### Run` section in the body (if provided)
- `depends_on` based on `needs`

## How execution works

- The runner loads tasks with `workflow: <id>`.
- A step is runnable if it is open and has no blocking deps.
- Steps execute in the template's order (manifest-backed), one at a time.
- Steps with no `### Run` are agent-driven (manual gates).

## Merge workflow

`merge.workflow.toml` is a repo-level template wired to `scripts/merge/*.sh` steps.
Build it with `crank build merge --id <workflow-id>` and run with `crank run --workflow <id>`.

Key steps (in order):
- `preflight`: clean tree, ahead of base
- `pre-merge` and `review` run in parallel
- `conflicts`: `git merge-tree` check
- `approval`: optional (only if `--notify`)
- `merge`: merge + push
- `tutorial`: generate post-merge tutorial (best-effort)

## Release workflow

`release.workflow.toml` provides a 5-step release pipeline (prep, checks, tag, publish, verify). Apply and run it like any other workflow.

## Review behavior when no task exists

The review prompt first tries to find an in-progress task. If none exists, it falls back to reviewing based on `git diff` and the user request. So workflows can run review without a task file.

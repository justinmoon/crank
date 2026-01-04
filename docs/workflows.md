# Workflows (Agent Overview)

Workflows are just graphs of tasks. A workflow template defines steps and dependencies, and `crank` expands it into `.crank/*.md` tasks that the runner executes. This lets agents follow a clear DAG without digging into implementation details.

## Mental model

- Template = blueprint (steps + needs)
- Instance = concrete run (tasks + status)
- Execution = run all ready steps, in parallel where possible

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
run = "crank merge-step preflight --base {{base}}"

[[steps]]
id = "review"
title = "Run review"
run = "crank merge-step review"
needs = ["preflight"]
```

## Commands (what an agent needs)

- List templates: `crank workflow list`
- Apply template: `crank workflow apply <template> --id <workflow-id> --var key=val`
- Run workflow: `crank workflow run <workflow-id> [--concurrency N]`

Apply creates tasks in `.crank/` like:
- `workflow: <workflow-id>`
- `step_id: <step-id>`
- `run: <command>` (if provided)
- `depends_on` based on `needs`

## How execution works

- The runner loads tasks with `workflow: <id>`.
- A step is runnable if it is open, has a `run` command, and has no blocking deps.
- Runnable steps execute concurrently (bounded by `--concurrency`).
- Steps with no `run` are manual gates. The runner stops and prints them as "waiting".

## Merge workflow

`crank merge` now instantiates and runs the `merge` workflow template. It is the only supported path for merging.

Key steps (in order):
- `preflight`: clean tree, ahead of base
- `pre-merge` and `review` run in parallel
- `conflicts`: `git merge-tree` check
- `approval`: optional (only if `--notify`)
- `merge`: merge + push

## Release workflow

`release.workflow.toml` provides a 5-step release pipeline (prep, checks, tag, publish, verify). Apply and run it like any other workflow.

## Review behavior when no task exists

The review prompt first tries to find an in-progress task. If none exists, it falls back to reviewing based on `git diff` and the user request. So workflows can run review without a task file.

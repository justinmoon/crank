# crank

`crank` is an unattended governor for multi-plan coding runs.

It runs a single orchestrator loop, persists state to disk, and keeps going until all tasks are terminal (`completed` or `blocked_best_effort`).

## Commands

- `cargo run -- run --config <file>`
- `cargo run -- init --output <file>`
- `cargo run -- ctl snapshot --state-dir <dir>`
- `cargo run -- ctl can-exit --state-dir <dir>`
- `cargo run -- ctl note --state-dir <dir> --message "..."`

## Config Highlights

Top-level fields:

- `run_id` (optional)
- `workspace`
- `state_dir`
- `unattended`
- `poll_interval_secs`
- `[timeouts] stall_secs`
- `[recovery] max_recovery_attempts_per_task, max_failures_before_block, backoff_initial_secs, backoff_max_secs`
- `[backend]` (`kind = "codex"` or `"mock"`)
- `[roles.implementer|reviewer_1|reviewer_2]` with `harness/model/thinking`
- `[[tasks]]` with `id`, `todo_file`, `depends_on`, optional `coord_dir`, optional `completion_file`

Task completion defaults to: `<coord_dir>/state.md` equals `done`.

If `completion_file` is set on a task, existence of that file marks completion.

## Example Test Run

Mock backend example:

```bash
cargo run -- run --config examples/mock-run.toml
cargo run -- ctl snapshot --state-dir runs/mock-call-plans
cargo run -- ctl can-exit --state-dir runs/mock-call-plans
```

This example validates dependency ordering across 4 tasks and the completion gate.

## Prompt Templates

Prompt text is stored in `prompts/*.md` and embedded into the binary via `include_str!`.
This keeps prompt editing readable and allows simple `{{placeholder}}` templating in Rust.

## Nix / Flake

This repo now includes a flake with a Rust dev shell and reproducible checks:

```bash
nix develop
nix flake check
```

`nix flake check` runs:

- package build
- `cargo test --frozen --locked`
- `cargo fmt --all -- --check`
- `cargo clippy --frozen --all-targets`

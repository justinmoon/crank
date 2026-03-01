# crank

`crank` is an unattended governor for multi-plan coding runs.

It runs a single orchestrator loop, persists state to disk, and keeps going until all tasks are terminal (`completed` or `blocked_best_effort`).

## Commands

- `cargo run -- run --config <file>`
- `cargo run -- run --config <file> --team xhigh`
- `cargo run -- init --output <file>`
- `cargo run -- init --output <file> --team xhigh`
- `cargo run -- ctl snapshot --state-dir <dir>`
- `cargo run -- ctl can-exit --state-dir <dir>`
- `cargo run -- ctl note --state-dir <dir> --message "..."`
- `cargo run -- teams list [--dir teams]`
- `cargo run -- teams validate --team <name>`
- `cargo run -- teams validate --all`

## Config Highlights

Top-level fields:

- `run_id` (optional)
- `workspace`
- `state_dir`
- `unattended`
- `poll_interval_secs`
- `[timeouts] stall_secs`
- `[recovery] max_recovery_attempts_per_task, max_failures_before_block, backoff_initial_secs, backoff_max_secs`
- `[backend]` (`kind = "codex" | "claude" | "droid" | "pi" | "mock"`)
- `[roles.implementer|reviewer_1|reviewer_2]` with `harness/model/thinking`
  - each role also supports `launch_args = ["..."]`
- `[[tasks]]` with `id`, `todo_file`, `depends_on`, optional `coord_dir`, optional `completion_file`

Role launch-arg policy is enforced by validation:

- `harness = "codex"` must include `launch_args = ["--yolo", ...]`
- `harness = "claude"` must include `launch_args = ["--dangerously-skip-permissions", ...]`

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

## Teams

Store reusable team definitions in `teams/*.toml`, then use:

```bash
cargo run -- teams list
cargo run -- teams validate --team xhigh
cargo run -- run --config /tmp/crank.toml --team xhigh
```

Builtin team:
- `xhigh` (codex implementer + codex reviewer-1 + claude reviewer-2, all `xhigh`)

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

## Local Harness E2E (Ignored)

These smoke tests are intentionally ignored in CI because they require local auth/session state.

```bash
cargo test local_e2e_claude_backend_smoke -- --ignored --nocapture
cargo test local_e2e_droid_backend_smoke -- --ignored --nocapture
cargo test local_e2e_pi_backend_smoke -- --ignored --nocapture
```

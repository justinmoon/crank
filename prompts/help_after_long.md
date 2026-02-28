Agent Quickstart:
  1. Create a starter config:
       crank init --output /tmp/crank.toml
  2. Edit the config:
       - set workspace + state_dir
       - choose backend + role models
       - add tasks with todo_file and dependencies
  3. Run the governor:
       crank run --config /tmp/crank.toml
  4. Inspect state and progress:
       crank ctl snapshot --state-dir <state_dir>
  5. Check if safe to stop:
       crank ctl can-exit --state-dir <state_dir>

Run from source:
  cargo run -- --help
  cargo run -- run --config /tmp/crank.toml
  cargo run -- teams list
  cargo run -- teams validate --team xhigh

Quality loop:
  - Execute the plan step-by-step.
  - Require review/checkpoint signal before advancing.
  - Fix serious workflow/reliability issues encountered during execution.

Default role contract:
  - Crank internally enforces implementer/reviewer todo workflow defaults.
  - User prompts can stay short (e.g. "implement <todo> with crank").

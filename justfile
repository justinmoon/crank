set shell := ["zsh", "-cu"]

default:
  @just --list

fmt:
  cargo fmt --all

test:
  cargo test

clippy:
  cargo clippy --all-targets

check: fmt test clippy

run config:
  cargo run -- run --config {{config}}

init output:
  cargo run -- init --output {{output}}

teams-list:
  cargo run -- teams list

teams-validate team:
  cargo run -- teams validate --team {{team}}

local-e2e:
  cargo test local_e2e_ -- --ignored --nocapture

local-e2e-claude:
  cargo test local_e2e_claude_backend_smoke -- --ignored --nocapture

local-e2e-droid:
  cargo test local_e2e_droid_backend_smoke -- --ignored --nocapture

local-e2e-pi:
  cargo test local_e2e_pi_backend_smoke -- --ignored --nocapture

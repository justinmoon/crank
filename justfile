set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

# Check for nix shell
[private]
nix-check:
    @test -n "$IN_NIX_SHELL" || (echo "Run 'nix develop' first" && exit 1)

# List available commands
default:
    @just --list

# Run dev build
dev: nix-check
    cargo build

# Build release
build: nix-check
    cargo build --release

# Type check
check: nix-check
    cargo check

# Run tests
test: nix-check
    cargo test

# Format code
format: nix-check
    cargo fmt

# Lint (clippy + format check)
lint: nix-check
    cargo fmt --check
    cargo clippy -- -D warnings

# Run E2E tests (requires release build)
test-e2e: nix-check build
    ./tests/e2e/test-merge.sh
    ./tests/e2e/test-workflow.sh

# Pre-merge checks
pre-merge: check lint test build test-e2e
    @echo "All checks passed!"

# Install to ~/.cargo/bin
install: nix-check
    cargo install --path .

# Nightly tasks
nightly: nix-check
    cargo audit || true

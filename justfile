set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

# Check for nix shell
[private]
nix-check:
    @test -n "${IN_NIX_SHELL:-}" || (echo "Run 'nix develop' first" && exit 1)

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
    ./tests/e2e/test-land.sh
    ./tests/e2e/test-workflow.sh

# Pre-merge checks (run before landing)
pre-merge: check lint test build test-e2e
    @echo "All pre-merge checks passed!"

# Install via nix (pushes current commit, updates ~/configs flake, rebuilds)
install:
    #!/usr/bin/env bash
    set -euo pipefail
    
    # Ensure we have a clean commit to push
    if [[ -n "$(git status --porcelain)" ]]; then
        echo "Error: uncommitted changes. Commit first."
        exit 1
    fi
    
    COMMIT=$(git rev-parse HEAD)
    BRANCH=$(git rev-parse --abbrev-ref HEAD)
    
    # Push current branch to github
    echo "Pushing $BRANCH to github..."
    git push origin "$BRANCH"
    
    # Update ~/configs flake.lock to use this commit
    echo "Updating ~/configs flake to crank@$COMMIT..."
    cd ~/configs
    nix flake lock --update-input crank --override-input crank "github:justinmoon/crank/$COMMIT"
    
    # Rebuild (darwin-rebuild or nixos-rebuild based on OS)
    echo "Rebuilding system..."
    if [[ "$(uname)" == "Darwin" ]]; then
        darwin-rebuild switch --flake .
    else
        sudo nixos-rebuild switch --flake .
    fi
    
    echo "Done! crank installed from commit $COMMIT"

# Nightly tasks
nightly: nix-check
    cargo audit || true

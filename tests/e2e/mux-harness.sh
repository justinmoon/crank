#!/bin/bash
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
CRANK_BIN="${CRANK_BIN:-$PROJECT_ROOT/target/release/crank}"

ensure_crank_bin() {
    if [[ ! -x "$CRANK_BIN" ]]; then
        echo -e "${RED}ERROR: crank binary not found at $CRANK_BIN${NC}"
        echo "Run 'cargo build --release' first"
        exit 1
    fi
}

pass() {
    echo -e "${GREEN}PASS${NC}: $1"
}

fail() {
    echo -e "${RED}FAIL${NC}: $1"
}

skip() {
    echo -e "${YELLOW}SKIP${NC}: $1"
}

require_bin() {
    local bin="$1"
    if ! command -v "$bin" >/dev/null 2>&1; then
        skip "$bin not installed"
        return 1
    fi
    return 0
}

create_mock_bin() {
    local bin_dir="$1"
    mkdir -p "$bin_dir"

    cat > "$bin_dir/codex" <<'EOF'
#!/bin/bash
set -euo pipefail
task_id="${CRANK_TASK_ID:-}"
if [[ -n "$task_id" ]]; then
  mkdir -p "$HOME/.crank/merged"
  echo "merged" > "$HOME/.crank/merged/$task_id"
fi
sleep 1
EOF
    chmod +x "$bin_dir/codex"

    cat > "$bin_dir/claude" <<'EOF'
#!/bin/bash
set -euo pipefail
sleep 1
EOF
    chmod +x "$bin_dir/claude"

    cat > "$bin_dir/direnv" <<'EOF'
#!/bin/bash
set -euo pipefail
exit 0
EOF
    chmod +x "$bin_dir/direnv"

    cat > "$bin_dir/llm" <<'EOF'
#!/bin/bash
set -euo pipefail
echo "e2e-branch"
EOF
    chmod +x "$bin_dir/llm"
}

setup_repo() {
    local repo="$1"
    local origin="$2"
    local project="$3"
    local task_count="$4"

    git init --bare "$origin" >/dev/null 2>&1
    git init "$repo" --initial-branch=master >/dev/null 2>&1
    git -C "$repo" config user.email "test@test.com"
    git -C "$repo" config user.name "Test User"
    echo "initial" > "$repo/README.md"
    git -C "$repo" add README.md
    git -C "$repo" commit -m "init" >/dev/null 2>&1
    git -C "$repo" remote add origin "$origin"
    git -C "$repo" push -u origin master >/dev/null 2>&1

    mkdir -p "$repo/.crank"
    for i in $(seq 1 "$task_count"); do
        local id
        printf -v id "pm%03d" "$i"
        cat > "$repo/.crank/${id}.md" <<EOF
---
app: ${project}
title: Define password manager requirements and threat model (${i})
priority: 5
status: open
supervision: unsupervised
coding_agent: codex
created: 2025-01-01
---

## Intent
Provide clear goals, scope, and security assumptions.

## Spec
- Outline core features and constraints.
- Identify threat actors and assets to protect.
EOF
    done

    mkdir -p "$repo/projects/crank/scripts"
    cat > "$repo/projects/crank/scripts/codex-notify" <<'EOF'
#!/bin/bash
exit 0
EOF
    chmod +x "$repo/projects/crank/scripts/codex-notify"
}

read_status() {
    local path="$1"
    grep -m1 '^status:' "$path" | awk '{print $2}'
}

wait_for_status() {
    local path="$1"
    local desired="$2"
    local timeout="${3:-40}"
    local start
    start=$(date +%s)

    while true; do
        if [[ -f "$path" ]]; then
            local current
            current=$(read_status "$path" || true)
            if [[ "$current" == "$desired" ]]; then
                return 0
            fi
        fi
        if (( $(date +%s) - start > timeout )); then
            return 1
        fi
        sleep 1
    done
}

count_worktrees() {
    local worktrees_dir="$1"
    local count=0
    if [[ -d "$worktrees_dir" ]]; then
        for entry in "$worktrees_dir"/*; do
            if [[ -d "$entry" ]]; then
                count=$((count + 1))
            fi
        done
    fi
    echo "$count"
}

wait_for_worktrees() {
    local worktrees_dir="$1"
    local expected="$2"
    local timeout="${3:-30}"
    local start
    start=$(date +%s)

    while true; do
        local count
        count=$(count_worktrees "$worktrees_dir")
        if [[ "$count" -ge "$expected" ]]; then
            return 0
        fi
        if (( $(date +%s) - start > timeout )); then
            return 1
        fi
        sleep 1
    done
}

find_task_in_worktrees() {
    local worktrees_dir="$1"
    local task_id="$2"
    if [[ -d "$worktrees_dir" ]]; then
        for entry in "$worktrees_dir"/*; do
            if [[ -f "$entry/.crank/${task_id}.md" ]]; then
                echo "$entry"
                return 0
            fi
        done
    fi
    return 1
}

run_mux_test() {
    local mux="$1"
    local concurrency="${2:-1}"

    ensure_crank_bin
    if ! require_bin "$mux"; then
        return 0
    fi

    local tmpdir
    tmpdir=$(mktemp -d)
    local repo="$tmpdir/repo"
    local origin="$tmpdir/origin.git"
    local project="e2e-$(date +%s)-$$"
    local session="crank(${project})"
    local bin_dir="$tmpdir/bin"
    local home_dir="$tmpdir/home"
    mkdir -p "$home_dir"

    export HOME="$home_dir"
    export TASK_BRANCH_MODEL="mock"
    create_mock_bin "$bin_dir"
    export PATH="$bin_dir:$PATH"

    setup_repo "$repo" "$origin" "$project" "$concurrency"

    E2E_MUX="$mux"
    E2E_SESSION="$session"
    E2E_TMPDIR="$tmpdir"
    if [[ "$mux" == "tmux" ]]; then
        export TMUX_TMPDIR="$tmpdir/tmux"
        mkdir -p "$TMUX_TMPDIR"
    fi

    cleanup() {
        if [[ "${CRANK_E2E_KEEP_TMP:-}" == "1" ]]; then
            echo "Keeping temp dir: ${E2E_TMPDIR:-}"
            return
        fi
        if [[ "${E2E_MUX:-}" == "tmux" ]]; then
            tmux kill-session -t "${E2E_SESSION:-}" >/dev/null 2>&1 || true
        else
            zellij kill-session "${E2E_SESSION:-}" >/dev/null 2>&1 || true
        fi
        rm -rf "${E2E_TMPDIR:-}"
    }
    trap cleanup EXIT

    unset TMUX TMUX_PANE ZELLIJ_SESSION_NAME ZELLIJ_PANE_ID

    echo -e "${YELLOW}Starting crank ${mux} session...${NC}"
    if [[ "$mux" == "tmux" ]]; then
        (cd "$repo" && CRANK_TMUX_NO_ATTACH=1 "$CRANK_BIN" "$mux" -c "$concurrency" --mode unsupervised --project "$project") >/dev/null 2>&1
    else
        (cd "$repo" && "$CRANK_BIN" "$mux" -c "$concurrency" --mode unsupervised --project "$project") >/dev/null 2>&1
    fi

    for i in $(seq 1 "$concurrency"); do
        local id
        printf -v id "pm%03d" "$i"
        local task_path="$repo/.crank/${id}.md"
        if wait_for_status "$task_path" "closed" 60; then
            pass "$mux worker closed task ${id}"
        else
            fail "$mux worker did not close task ${id}"
            return 1
        fi
    done

    if wait_for_worktrees "$repo/worktrees" "$concurrency" 40; then
        for i in $(seq 1 "$concurrency"); do
            local id
            printf -v id "pm%03d" "$i"
            if find_task_in_worktrees "$repo/worktrees" "$id" >/dev/null; then
                pass "$mux copied task file into worktree for ${id}"
            else
                fail "$mux missing copied task file in worktree for ${id}"
                return 1
            fi
        done
    else
        fail "$mux worktrees not created"
        return 1
    fi

    for i in $(seq 1 "$concurrency"); do
        if [[ -f "$HOME/.crank/logs/worker-${i}.log" ]]; then
            pass "$mux wrote worker-${i} log"
        else
            fail "$mux worker-${i} log missing"
            return 1
        fi
    done
}

#!/bin/bash
# E2E tests for crank merge command
# Tests the core merge workflow without OpenCode or project-specific CI
set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Get the crank binary path
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
CRANK_BIN="${CRANK_BIN:-$PROJECT_ROOT/target/release/crank}"

# Check if crank binary exists
if [[ ! -x "$CRANK_BIN" ]]; then
    echo -e "${RED}ERROR: crank binary not found at $CRANK_BIN${NC}"
    echo "Run 'cargo build --release' first"
    exit 1
fi

export PATH="$(dirname "$CRANK_BIN"):$PATH"

# Create temp directory for test repos
TMPDIR=$(mktemp -d)
trap "rm -rf $TMPDIR" EXIT

echo -e "${YELLOW}Using temp directory: $TMPDIR${NC}"
echo -e "${YELLOW}Using crank binary: $CRANK_BIN${NC}"
echo ""

# Track test results
TESTS_RUN=0
TESTS_PASSED=0
TESTS_FAILED=0

pass() {
    echo -e "${GREEN}PASS${NC}: $1"
    ((TESTS_PASSED++)) || true
}

fail() {
    echo -e "${RED}FAIL${NC}: $1"
    ((TESTS_FAILED++)) || true
}

# ============================================================================
# Setup: Create bare origin and initial commit
# ============================================================================
setup_repos() {
    echo -e "${YELLOW}Setting up test repositories...${NC}"
    
    # Create bare origin repo
    git init --bare "$TMPDIR/origin" --initial-branch=master >/dev/null 2>&1
    
    # Create worktree clone
    git clone "$TMPDIR/origin" "$TMPDIR/worktree" >/dev/null 2>&1
    
    # Create target repo clone  
    git clone "$TMPDIR/origin" "$TMPDIR/target" >/dev/null 2>&1
    
    # Create initial commit on master
    cd "$TMPDIR/worktree"
    git config user.email "test@test.com"
    git config user.name "Test User"
    echo "initial content" > file.txt
    git add file.txt
    git commit -m "initial commit" >/dev/null 2>&1
    git push origin master >/dev/null 2>&1
    
    # Configure target repo
    cd "$TMPDIR/target"
    git config user.email "test@test.com"
    git config user.name "Test User"
    git fetch origin >/dev/null 2>&1

    mkdir -p "$TMPDIR/worktree/.crank/workflows"
    cat > "$TMPDIR/worktree/.crank/workflows/merge.workflow.toml" <<'EOF'
workflow = "merge"
version = 1

[vars]
base = { default = "master" }
worktree = { default = "." }
timeout = { default = "600000" }
notify_interval = { default = "60000" }
skip_pre_merge_flag = { default = "" }
skip_review_flag = { default = "" }
review_skip_tests_flag = { default = "" }
dry_run_flag = { default = "" }
notify_flag = { default = "" }
target_repo_flag = { default = "" }

[[steps]]
id = "preflight"
title = "Preflight checks"
run = "crank merge-step preflight --worktree \"{{worktree}}\" --base {{base}}"

[[steps]]
id = "pre-merge"
title = "Run pre-merge"
run = "crank merge-step pre-merge --worktree \"{{worktree}}\" --timeout {{timeout}} {{skip_pre_merge_flag}}"
needs = ["preflight"]

[[steps]]
id = "review"
title = "Run review"
run = "crank merge-step review --worktree \"{{worktree}}\" --timeout {{timeout}} {{review_skip_tests_flag}} {{skip_review_flag}}"
needs = ["preflight"]

[[steps]]
id = "conflicts"
title = "Check conflicts"
run = "crank merge-step conflicts --worktree \"{{worktree}}\" --base {{base}}"
needs = ["pre-merge", "review"]

[[steps]]
id = "approval"
title = "Wait for approval"
run = "crank merge-step approval --worktree \"{{worktree}}\" --base {{base}} {{notify_flag}} --notify-interval {{notify_interval}} {{target_repo_flag}}"
needs = ["conflicts"]

[[steps]]
id = "merge"
title = "Merge and push"
run = "crank merge-step apply --worktree \"{{worktree}}\" --base {{base}} {{dry_run_flag}} {{target_repo_flag}}"
needs = ["approval"]
EOF

    echo ".crank/workflows/" >> "$TMPDIR/worktree/.git/info/exclude"

    echo "Repositories ready."
    echo ""
}

# ============================================================================
# Test 1: Happy path merge
# ============================================================================
test_happy_path_merge() {
    ((TESTS_RUN++)) || true
    echo -e "${YELLOW}Test 1: Happy path merge${NC}"
    
    cd "$TMPDIR/worktree"
    
    # Create feature branch with change
    git checkout -b feature-add-line >/dev/null 2>&1
    echo "new feature line" >> file.txt
    git commit -am "add feature line" >/dev/null 2>&1
    
    # Run crank merge
    local output
    if output=$("$CRANK_BIN" merge "$TMPDIR/worktree" \
        --skip-review \
        --skip-pre-merge \
        --target-repo "$TMPDIR/target" \
        --base master 2>&1); then
        # Verify merge landed on origin
        cd "$TMPDIR/origin"
        if git log --oneline master | grep -q "Merge feature-add-line"; then
            pass "Branch merged and pushed to origin/master"
        else
            fail "Merge commit not found on origin/master"
            echo "  Origin log: $(git log --oneline -3 master)"
        fi
    else
        fail "crank merge command failed"
        echo "  Output: $output"
    fi
    
    # Cleanup: go back to master
    cd "$TMPDIR/worktree"
    git checkout master >/dev/null 2>&1
    git pull origin master >/dev/null 2>&1
    echo ""
}

# ============================================================================
# Test 2: Preflight requires commits and clean tree
# ============================================================================
test_preflight_requires_commit() {
    ((TESTS_RUN+=2)) || true
    echo -e "${YELLOW}Test 2: Preflight requires commits and clean tree${NC}"

    cd "$TMPDIR/worktree"
    git checkout master >/dev/null 2>&1
    git fetch origin >/dev/null 2>&1
    git reset --hard origin/master >/dev/null 2>&1

    local output
    local exit_code=0
    output=$("$CRANK_BIN" merge "$TMPDIR/worktree" \
        --skip-review \
        --skip-pre-merge \
        --target-repo "$TMPDIR/target" \
        --base master 2>&1) || exit_code=$?

    if [[ $exit_code -ne 0 ]] \
        && echo "$output" | grep -q "no commits to merge"; then
        pass "Preflight blocks merges with no commits"
    else
        fail "Expected preflight failure for missing commits"
        echo "  Exit code: $exit_code"
        echo "  Output: $output"
    fi

    git checkout -b feature-dirty >/dev/null 2>&1
    echo "dirty change" >> file.txt

    exit_code=0
    output=$("$CRANK_BIN" merge "$TMPDIR/worktree" \
        --skip-review \
        --skip-pre-merge \
        --target-repo "$TMPDIR/target" \
        --base master 2>&1) || exit_code=$?

    if [[ $exit_code -ne 0 ]] \
        && echo "$output" | grep -q "uncommitted changes"; then
        pass "Preflight blocks merges with dirty worktree"
    else
        fail "Expected preflight failure for dirty worktree"
        echo "  Exit code: $exit_code"
        echo "  Output: $output"
    fi

    git reset --hard >/dev/null 2>&1
    git checkout master >/dev/null 2>&1
    echo ""
}

# ============================================================================
# Test 3: Conflict detection
# ============================================================================
test_conflict_detection() {
    ((TESTS_RUN++)) || true
    echo -e "${YELLOW}Test 3: Conflict detection${NC}"
    
    # First, sync worktree to current origin/master
    cd "$TMPDIR/worktree"
    git checkout master >/dev/null 2>&1
    git fetch origin >/dev/null 2>&1
    git reset --hard origin/master >/dev/null 2>&1
    
    # Create a feature branch FROM current master with a change to line 1
    git checkout -b feature-conflict >/dev/null 2>&1
    echo "feature wants this content" > file.txt
    git commit -am "feature: change file content" >/dev/null 2>&1
    
    # Now push a DIFFERENT change to master via target repo
    # This creates the conflict: master and feature both modify file.txt differently
    cd "$TMPDIR/target"
    git checkout master >/dev/null 2>&1
    git fetch origin >/dev/null 2>&1
    git reset --hard origin/master >/dev/null 2>&1
    echo "master wants different content" > file.txt
    git commit -am "master: different change to file" >/dev/null 2>&1
    git push origin master >/dev/null 2>&1
    
    # Now try to merge feature-conflict - should detect conflict
    # because both branches modified file.txt differently
    cd "$TMPDIR/worktree"
    
    local output
    local exit_code=0
    output=$("$CRANK_BIN" merge "$TMPDIR/worktree" \
        --skip-review \
        --skip-pre-merge \
        --target-repo "$TMPDIR/target" \
        --base master 2>&1) || exit_code=$?
    
    # Should fail with conflict status
    if [[ $exit_code -ne 0 ]] && echo "$output" | grep -qi "merge conflict"; then
        pass "Conflict correctly detected and reported"
    else
        fail "Expected conflict error with non-zero exit"
        echo "  Exit code: $exit_code"
        echo "  Output: $output"
    fi
    
    # Cleanup
    cd "$TMPDIR/worktree"
    git checkout master >/dev/null 2>&1
    echo ""
}

# ============================================================================
# Test 4: Dry run doesn't merge
# ============================================================================
test_dry_run() {
    ((TESTS_RUN++)) || true
    echo -e "${YELLOW}Test 4: Dry run doesn't actually merge${NC}"
    
    cd "$TMPDIR/worktree"
    
    # Get current origin/master commit
    cd "$TMPDIR/origin"
    local before_commit
    before_commit=$(git rev-parse master)
    
    # Create new feature branch in worktree
    cd "$TMPDIR/worktree"
    git fetch origin >/dev/null 2>&1
    git checkout master >/dev/null 2>&1
    git reset --hard origin/master >/dev/null 2>&1
    git checkout -b feature-dry-run >/dev/null 2>&1
    echo "dry run test" >> file.txt
    git commit -am "dry run feature" >/dev/null 2>&1
    
    # Run crank merge with --dry-run
    local output
    if output=$("$CRANK_BIN" merge "$TMPDIR/worktree" \
        --skip-review \
        --skip-pre-merge \
        --target-repo "$TMPDIR/target" \
        --base master \
        --dry-run 2>&1); then
        # Verify origin/master unchanged
        cd "$TMPDIR/origin"
        local after_commit
        after_commit=$(git rev-parse master)

        if [[ "$before_commit" == "$after_commit" ]]; then
            pass "Dry run did not modify origin/master"
        else
            fail "Dry run incorrectly modified origin/master"
            echo "  Before: $before_commit"
            echo "  After: $after_commit"
        fi
    else
        fail "crank merge --dry-run command failed"
        echo "  Output: $output"
    fi
    
    # Cleanup
    cd "$TMPDIR/worktree"
    git checkout master >/dev/null 2>&1
    echo ""
}

# ============================================================================
# Test 5: Approval workflow (pending/approve/reject)
# ============================================================================
test_approval_workflow() {
    ((TESTS_RUN++)) || true
    echo -e "${YELLOW}Test 5: Approval workflow commands${NC}"
    
    # Test pending command (should show no pending)
    local output
    output=$("$CRANK_BIN" pending 2>&1)
    
    if echo "$output" | grep -q '"status":"ok"'; then
        pass "pending command works"
    else
        fail "pending command failed"
        echo "  Output: $output"
    fi
    echo ""
}

# ============================================================================
# Main
# ============================================================================
main() {
    echo "============================================"
    echo "crank E2E Tests"
    echo "============================================"
    echo ""
    
    setup_repos
    
    test_happy_path_merge
    test_preflight_requires_commit
    test_conflict_detection
    test_dry_run
    test_approval_workflow
    
    echo "============================================"
    echo "Results: $TESTS_PASSED/$TESTS_RUN passed"
    echo "============================================"
    
    if [[ $TESTS_FAILED -gt 0 ]]; then
        echo -e "${RED}$TESTS_FAILED test(s) failed${NC}"
        exit 1
    else
        echo -e "${GREEN}All tests passed!${NC}"
        exit 0
    fi
}

main "$@"

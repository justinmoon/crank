#!/bin/bash
# E2E tests for crank land workflow templates
# Tests the core land workflow without OpenCode or project-specific CI
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
export CRANK_RUN_NO_AGENT=1

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

build_land_workflow() {
    local workflow_id="$1"
    local worktree="$2"
    local target_repo="$3"
    local dry_run_flag="${4:-}"
    local skip_pre_merge_flag="${5:---skip}"
    local skip_review_flag="${6:---skip}"
    local review_skip_tests_flag="${7:-}"
    local notify_flag="${8:-}"

    local vars=(
        --var "base=master"
        --var "worktree=$worktree"
        --var "timeout=600000"
        --var "notify_interval=60000"
        --var "skip_pre_merge_flag=$skip_pre_merge_flag"
        --var "skip_review_flag=$skip_review_flag"
        --var "review_skip_tests_flag=$review_skip_tests_flag"
        --var "dry_run_flag=$dry_run_flag"
        --var "notify_flag=$notify_flag"
    )

    if [[ -n "$target_repo" ]]; then
        vars+=(--var "target_repo_flag=--target-repo $target_repo")
    else
        vars+=(--var "target_repo_flag=")
    fi

    "$CRANK_BIN" build land --id "$workflow_id" --ephemeral "${vars[@]}"
}

run_workflow_until_done() {
    local workflow_id="$1"
    local max_attempts=20
    local output

    for _ in $(seq 1 "$max_attempts"); do
        output=$("$CRANK_BIN" run --workflow "$workflow_id" 2>&1) || {
            echo "$output"
            return 1
        }
        if echo "$output" | grep -q "Workflow '${workflow_id}' complete"; then
            return 0
        fi
    done

    echo "workflow did not complete"
    return 1
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
    cp "$PROJECT_ROOT/.crank/workflows/land.workflow.toml" "$TMPDIR/worktree/.crank/workflows/land.workflow.toml"

    mkdir -p "$TMPDIR/worktree/scripts"
    cp -R "$PROJECT_ROOT/scripts/land" "$TMPDIR/worktree/scripts/land"
    chmod +x "$TMPDIR/worktree/scripts/land/"*.sh

    echo ".crank/workflows/" >> "$TMPDIR/worktree/.git/info/exclude"
    echo "scripts/" >> "$TMPDIR/worktree/.git/info/exclude"

    echo "Repositories ready."
    echo ""
}

# ============================================================================
# Test 1: Happy path land
# ============================================================================
test_happy_path_land() {
    ((TESTS_RUN++)) || true
    echo -e "${YELLOW}Test 1: Happy path land${NC}"

    cd "$TMPDIR/worktree"

    # Create feature branch with change
    git checkout -b feature-add-line >/dev/null 2>&1
    echo "new feature line" >> file.txt
    git commit -am "add feature line" >/dev/null 2>&1

    local workflow_id="land-happy"
    if ! build_land_workflow "$workflow_id" "$TMPDIR/worktree" "$TMPDIR/target" "" "--skip" "--skip"; then
        fail "build failed"
        return
    fi

    if run_workflow_until_done "$workflow_id"; then
        cd "$TMPDIR/origin"
        if git log --oneline master | grep -q "Land feature-add-line"; then
            pass "Branch landed and pushed to origin/master"
        else
            fail "Land commit not found on origin/master"
            echo "  Origin log: $(git log --oneline -3 master)"
        fi
    else
        fail "land workflow failed"
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
    local workflow_id="land-preflight-1"
    if ! build_land_workflow "$workflow_id" "$TMPDIR/worktree" "$TMPDIR/target" "" "--skip" "--skip"; then
        fail "build failed"
        return
    fi

    for _ in {1..20}; do
        output=$("$CRANK_BIN" run --workflow "$workflow_id" 2>&1) || {
            exit_code=$?
            break
        }
        if echo "$output" | grep -q "Workflow '${workflow_id}' complete"; then
            break
        fi
    done

    if [[ $exit_code -ne 0 ]] \
        && echo "$output" | grep -q "no commits to land"; then
        pass "Preflight blocks lands with no commits"
    else
        fail "Expected preflight failure for missing commits"
        echo "  Exit code: $exit_code"
        echo "  Output: $output"
    fi

    git checkout -b feature-dirty >/dev/null 2>&1
    echo "dirty change" >> file.txt

    exit_code=0
    workflow_id="land-preflight-2"
    if ! build_land_workflow "$workflow_id" "$TMPDIR/worktree" "$TMPDIR/target" "" "--skip" "--skip"; then
        fail "build failed"
        return
    fi

    for _ in {1..20}; do
        output=$("$CRANK_BIN" run --workflow "$workflow_id" 2>&1) || {
            exit_code=$?
            break
        }
        if echo "$output" | grep -q "Workflow '${workflow_id}' complete"; then
            break
        fi
    done

    if [[ $exit_code -ne 0 ]] \
        && echo "$output" | grep -q "uncommitted changes"; then
        pass "Preflight blocks lands with dirty worktree"
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

    # Now try to land feature-conflict - should detect conflict
    # because both branches modified file.txt differently
    cd "$TMPDIR/worktree"

    local output
    local exit_code=0
    local workflow_id="land-conflict"
    if ! build_land_workflow "$workflow_id" "$TMPDIR/worktree" "$TMPDIR/target" "" "--skip" "--skip"; then
        fail "build failed"
        return
    fi

    for _ in {1..20}; do
        output=$("$CRANK_BIN" run --workflow "$workflow_id" 2>&1) || {
            exit_code=$?
            break
        }
        if echo "$output" | grep -q "Workflow '${workflow_id}' complete"; then
            break
        fi
    done

    # Should fail with conflict status
    if [[ $exit_code -ne 0 ]] && echo "$output" | grep -qi "conflict"; then
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
# Test 4: Dry run doesn't land
# ============================================================================
test_dry_run() {
    ((TESTS_RUN++)) || true
    echo -e "${YELLOW}Test 4: Dry run doesn't actually land${NC}"

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

    local workflow_id="land-dry-run"
    if ! build_land_workflow "$workflow_id" "$TMPDIR/worktree" "$TMPDIR/target" "--dry-run" "--skip" "--skip"; then
        fail "build failed"
        return
    fi

    local output
    if output=$(run_workflow_until_done "$workflow_id" 2>&1); then
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
        fail "dry run workflow failed"
        echo "  Output: $output"
    fi

    # Cleanup
    cd "$TMPDIR/worktree"
    git checkout master >/dev/null 2>&1
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

    test_happy_path_land
    test_preflight_requires_commit
    test_conflict_detection
    test_dry_run
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

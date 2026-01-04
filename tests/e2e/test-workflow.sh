#!/bin/bash
# E2E tests for crank build/run workflow command
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
CRANK_BIN="${CRANK_BIN:-$PROJECT_ROOT/target/release/crank}"

if [[ ! -x "$CRANK_BIN" ]]; then
    echo -e "${RED}ERROR: crank binary not found at $CRANK_BIN${NC}"
    echo "Run 'cargo build --release' first"
    exit 1
fi

export PATH="$(dirname "$CRANK_BIN"):$PATH"
export CRANK_RUN_NO_AGENT=1

TMPDIR=$(mktemp -d)
trap "rm -rf $TMPDIR" EXIT

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

setup_repo() {
    echo -e "${YELLOW}Setting up workflow repo...${NC}"
    git init "$TMPDIR/repo" --initial-branch=master >/dev/null 2>&1
    cd "$TMPDIR/repo"
    git config user.email "test@test.com"
    git config user.name "Test User"
    echo "initial" > README.md
    git add README.md
    git commit -m "init" >/dev/null 2>&1
    mkdir -p .crank/workflows
    echo ".crank/workflows/" >> .git/info/exclude
}

write_template() {
    local name="$1"
    local content="$2"
    printf "%b" "$content" > ".crank/workflows/${name}.workflow.toml"
}

read_status() {
    local id="$1"
    grep -m1 '^status:' ".crank/${id}.md" | awk '{print $2}'
}

# Test 1: Apply and run a workflow with concurrency
workflow_basic() {
    ((TESTS_RUN++)) || true
    echo -e "${YELLOW}Test 1: Apply and run workflow${NC}"

    cd "$TMPDIR/repo"

    local log="$TMPDIR/workflow.log"

    write_template "test" "workflow = \"test\"\nversion = 1\n\n[vars]\nlog = { required = true }\n\n[[steps]]\nid = \"preflight\"\ntitle = \"Preflight\"\nrun = \"echo preflight >> {{log}}\"\n\n[[steps]]\nid = \"step-a\"\ntitle = \"Step A\"\nrun = \"sleep 1; echo step-a >> {{log}}\"\nneeds = [\"preflight\"]\n\n[[steps]]\nid = \"step-b\"\ntitle = \"Step B\"\nrun = \"sleep 1; echo step-b >> {{log}}\"\nneeds = [\"preflight\"]\n\n[[steps]]\nid = \"join\"\ntitle = \"Join\"\nrun = \"echo join >> {{log}}\"\nneeds = [\"step-a\", \"step-b\"]\n"

    local output
    if output=$("$CRANK_BIN" build test --id wf-basic --var log="$log" 2>&1); then
        if [[ -f ".crank/wf-basic.preflight.md" ]] && [[ -f ".crank/wf-basic.join.md" ]]; then
            pass "Workflow tasks created"
        else
            fail "Workflow tasks not created"
        fi
    else
        fail "build failed"
        echo "  Output: $output"
        return
    fi

    local output
    local done=false
    for _ in {1..10}; do
        output=$("$CRANK_BIN" run --workflow wf-basic 2>&1) || {
            fail "run failed"
            echo "  Output: $output"
            return
        }
        if echo "$output" | grep -q "Workflow 'wf-basic' complete"; then
            done=true
            break
        fi
    done

    if [[ "$done" != "true" ]]; then
        fail "Workflow did not complete"
        return
    fi

    local first
    local last
    first=$(head -n 1 "$log" | tr -d '\r')
    last=$(tail -n 1 "$log" | tr -d '\r')
    if [[ "$first" == "preflight" ]] && [[ "$last" == "join" ]]; then
        pass "Workflow ran in dependency order"
    else
        fail "Workflow order incorrect"
    fi
    echo ""
}

# Test 2: Manual gate blocks execution
workflow_gate() {
    ((TESTS_RUN++)) || true
    echo -e "${YELLOW}Test 2: Manual gate handling${NC}"

    cd "$TMPDIR/repo"

    local log="$TMPDIR/gate.log"

    write_template "gate" "workflow = \"gate\"\nversion = 1\n\n[vars]\nlog = { required = true }\n\n[[steps]]\nid = \"gate\"\ntitle = \"Wait for approval\"\n\n[[steps]]\nid = \"after\"\ntitle = \"After gate\"\nrun = \"echo after >> {{log}}\"\nneeds = [\"gate\"]\n"

    local output
    if ! output=$("$CRANK_BIN" build gate --id wf-gate --var log="$log" 2>&1); then
        fail "build failed"
        echo "  Output: $output"
        return
    fi

    "$CRANK_BIN" run --workflow wf-gate >/dev/null 2>&1

    local status
    status=$(read_status "wf-gate.after")
    if [[ "$status" == "open" ]]; then
        pass "Manual gate blocks dependent step"
    else
        fail "Gate did not block dependent step"
    fi

    "$CRANK_BIN" task done wf-gate.gate >/dev/null 2>&1

    if "$CRANK_BIN" run --workflow wf-gate >/dev/null 2>&1 && grep -q "after" "$log"; then
        pass "Workflow resumes after gate closes"
    else
        fail "Workflow did not resume after gate"
    fi
    echo ""
}

# Test 3: Failure stops workflow
workflow_failure() {
    ((TESTS_RUN++)) || true
    echo -e "${YELLOW}Test 3: Failure handling${NC}"

    cd "$TMPDIR/repo"

    write_template "fail" "workflow = \"fail\"\nversion = 1\n\n[[steps]]\nid = \"boom\"\ntitle = \"Boom\"\nrun = \"false\"\n\n[[steps]]\nid = \"after\"\ntitle = \"After\"\nrun = \"echo after\"\nneeds = [\"boom\"]\n"

    local output
    if ! output=$("$CRANK_BIN" build fail --id wf-fail 2>&1); then
        fail "build failed"
        echo "  Output: $output"
        return
    fi

    if "$CRANK_BIN" run --workflow wf-fail >/dev/null 2>&1; then
        fail "run should have failed"
        return
    fi

    local status
    status=$(read_status "wf-fail.after")
    if [[ "$status" == "open" ]]; then
        pass "Workflow stops on failure"
    else
        fail "Dependent step should remain open after failure"
    fi
    echo ""
}

main() {
    echo "============================================"
    echo "crank Workflow E2E Tests"
    echo "============================================"
    echo ""

    setup_repo

    workflow_basic
    workflow_gate
    workflow_failure

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

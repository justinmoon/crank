#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "$SCRIPT_DIR/common.sh"

worktree="."
timeout_ms="600000"
skip="false"
skip_tests="false"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --worktree)
      worktree="$2"
      shift 2
      ;;
    --timeout)
      timeout_ms="$2"
      shift 2
      ;;
    --skip)
      skip="true"
      shift 1
      ;;
    --skip-tests)
      skip_tests="true"
      shift 1
      ;;
    *)
      die "unknown argument: $1"
      ;;
  esac
done

if [[ "$skip" == "true" ]]; then
  echo "PASS (review skipped)"
  exit 0
fi

worktree=$(resolve_worktree "$worktree")

args=("review" "$worktree" "--timeout" "$timeout_ms")
if [[ "$skip_tests" == "true" ]]; then
  args+=("--skip-tests")
fi

set +e
output=$(crank "${args[@]}" 2>&1)
status=$?
set -e

if [[ $status -eq 0 ]]; then
  echo "PASS"
  exit 0
fi

echo "FAIL: review failed"
exit $status

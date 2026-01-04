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

json_line=$(printf "%s" "$output" | tail -n 1)

parse_result=$(python - <<'PY'
import json,sys
try:
    data=json.loads(sys.argv[1])
except Exception:
    print("")
    print("")
    sys.exit(0)
status=data.get("status") or ""
reason=(data.get("reason") or "").replace("\n", " ").strip()
print(status)
print(reason)
PY
"$json_line")

review_status=$(printf "%s" "$parse_result" | sed -n '1p')
review_reason=$(printf "%s" "$parse_result" | sed -n '2p')

if [[ "$review_status" == "pass" ]]; then
  echo "PASS"
  exit 0
fi

if [[ $status -ne 0 ]]; then
  if [[ "$review_status" == "fail" && -n "$review_reason" ]]; then
    printf "FAIL: %s\n" "$review_reason"
  else
    echo "FAIL: review failed"
  fi
  exit $status
fi

echo "FAIL: review output invalid"
exit 1

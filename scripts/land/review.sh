#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "$SCRIPT_DIR/common.sh"

worktree="."
timeout_ms="600000"
skip="false"
# Default to skipping tests: pre-merge already runs them via `just pre-merge`.
skip_tests="true"

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
root=$(git_root "$worktree")

cd "$root"

ensure_local_crank() {
  if [[ -x "./target/release/crank" || -x "./target/debug/crank" ]]; then
    return 0
  fi

  if [[ -n "${IN_NIX_SHELL:-}" ]]; then
    cargo build >/dev/null
    return 0
  fi

  if command -v nix >/dev/null 2>&1; then
    nix develop -c cargo build >/dev/null
    return 0
  fi

  die "Run 'nix develop' first (or install nix)"
}

ensure_local_crank

crank_bin="./target/debug/crank"
if [[ -x "./target/release/crank" ]]; then
  crank_bin="./target/release/crank"
fi

args=("review" "$worktree" "--timeout" "$timeout_ms")
if [[ "$skip_tests" == "true" ]]; then
  args+=("--skip-tests")
fi

set +e
output=$($crank_bin "${args[@]}" 2>&1)
status=$?
set -e

CRANK_REVIEW_OUTPUT="$output" CRANK_REVIEW_EXIT="$status" python3 - <<'PY'
import json
import os
import re
import sys

raw = os.environ.get("CRANK_REVIEW_OUTPUT", "")
exit_code = int(os.environ.get("CRANK_REVIEW_EXIT", "1") or "1")

def trim_details(value, limit=2000):
    if value is None:
        return None
    text = str(value).strip()
    if not text:
        return None
    if len(text) > limit:
        return text[:limit].rstrip() + "\n...(truncated)"
    return text

def emit_fail(message, details=None):
    print(f"FAIL: {message[:200]}")
    details_text = trim_details(details)
    if details_text:
        print("Review output (truncated):")
        print(details_text)
    sys.exit(1)

# Prefer parsing `crank review` JSON output:
#   {"status":"pass"|"fail","reason":<string|null>}
parsed_status = None
reason = None
details = None
for line in raw.splitlines():
    line = line.strip()
    if not line:
        continue
    try:
        obj = json.loads(line)
    except Exception:
        continue

    if isinstance(obj, dict) and "status" in obj:
        parsed_status = obj.get("status")
        reason = obj.get("reason")
        details = obj.get("details") or obj.get("output")

if isinstance(parsed_status, str):
    status_norm = parsed_status.strip().lower()
    if status_norm == "pass" and exit_code == 0:
        print("PASS")
        sys.exit(0)
    if status_norm == "fail":
        msg = (reason or "review failed")
        msg = msg.strip() if isinstance(msg, str) else "review failed"
        emit_fail(msg, details)

# Fallback: treat output as plain text and scan.
text = raw.strip()
first_line = next((ln.strip() for ln in text.splitlines() if ln.strip()), "")
first_line_norm = re.sub(r"^`+|`+$", "", first_line).strip()

if exit_code == 0 and (first_line_norm == "PASS" or first_line_norm.startswith("PASS")):
    print("PASS")
    sys.exit(0)

m = re.match(r"^FAIL:\s*(.*)$", first_line_norm)
if m:
    msg = (m.group(1) or "review failed").strip()
    tail = "\n".join(text.splitlines()[1:]).strip()
    emit_fail(msg, tail)

if exit_code == 0 and re.search(r"\bPASS\b", text, flags=re.IGNORECASE):
    print("PASS")
    sys.exit(0)

m2 = re.search(r"FAIL:\s*([^\n\r]{0,200})", text)
if m2:
    msg = (m2.group(1) or "review failed").strip()
    emit_fail(msg, text)

if exit_code != 0:
    emit_fail("review command failed", text)

emit_fail("Could not parse review output", text)
PY

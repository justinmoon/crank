#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "$SCRIPT_DIR/common.sh"

worktree="."
timeout_ms="600000"
skip="false"

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
    *)
      die "unknown argument: $1"
      ;;
  esac
done

if [[ "$skip" == "true" ]]; then
  echo "Skipping pre-merge checks"
  exit 0
fi

worktree=$(resolve_worktree "$worktree")
root=$(git_root "$worktree")

cd "$root"

if ! command -v just >/dev/null 2>&1; then
  die "just is required to run pre-merge"
fi

if command -v timeout >/dev/null 2>&1; then
  timeout_sec=$((timeout_ms / 1000))
  if [[ "$timeout_sec" -le 0 ]]; then
    timeout_sec=1
  fi
  timeout "$timeout_sec" just pre-merge
else
  just pre-merge
fi

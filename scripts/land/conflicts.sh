#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "$SCRIPT_DIR/common.sh"

worktree="."
base="master"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --worktree)
      worktree="$2"
      shift 2
      ;;
    --base)
      base="$2"
      shift 2
      ;;
    *)
      die "unknown argument: $1"
      ;;
  esac
done

worktree=$(resolve_worktree "$worktree")
cd "$worktree"

git fetch origin "$base" >/dev/null 2>&1

base_ref="origin/$base"

tmpfile=$(mktemp)
trap 'rm -f "$tmpfile"' EXIT

set +e
git merge-tree --write-tree "$base_ref" HEAD >"$tmpfile" 2>&1
merge_status=$?
set -e

if grep -qi "conflict" "$tmpfile"; then
  conflict=$(grep -i "Merge conflict in" "$tmpfile" | head -n 1 | sed 's/.*Merge conflict in //')
  if [[ -n "$conflict" ]]; then
    die "merge conflict: $conflict"
  fi
  die "merge conflict detected"
fi

if [[ $merge_status -ne 0 ]]; then
  die "merge conflict detected"
fi

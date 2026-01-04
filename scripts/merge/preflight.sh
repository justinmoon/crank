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

dirty=$(git status --porcelain)
if [[ -n "$dirty" ]]; then
  die "worktree has uncommitted changes:\n$dirty"
fi

base_ref="origin/$base"

ahead=$(git rev-list --count "$base_ref..HEAD")
if [[ "$ahead" == "0" ]]; then
  if git merge-base --is-ancestor HEAD "$base_ref"; then
    echo "No commits to merge; branch already merged into $base_ref"
    exit 0
  fi
  die "no commits to merge; branch is not ahead of $base_ref"
fi

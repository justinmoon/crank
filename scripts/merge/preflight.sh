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

ahead=$(git rev-list --count "origin/$base..HEAD")
if [[ "${ahead}" == "0" ]]; then
  die "no commits to merge; commit changes before running crank merge"
fi

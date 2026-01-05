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
  current_branch=$(git symbolic-ref --quiet --short HEAD || true)

  # If you're on the base branch itself, landing makes no sense; fail fast.
  if [[ "$current_branch" == "$base" ]]; then
    die "no commits to land; branch is not ahead of $base_ref"
  fi

  # Allow already-landed feature branches to exit successfully.
  if git merge-base --is-ancestor HEAD "$base_ref"; then
    echo "No commits to land; branch already landed in $base_ref"
    exit 0
  fi

  die "no commits to land; branch is not ahead of $base_ref"
fi

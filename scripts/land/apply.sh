#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "$SCRIPT_DIR/common.sh"

worktree="."
base="master"
dry_run="false"
target_repo=""

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
    --dry-run)
      dry_run="true"
      shift 1
      ;;
    --target-repo)
      target_repo="$2"
      shift 2
      ;;
    *)
      die "unknown argument: $1"
      ;;
  esac
done

if [[ "$dry_run" == "true" ]]; then
  echo "Dry run: land skipped"
  exit 0
fi

worktree=$(resolve_worktree "$worktree")
source_branch=$(current_branch "$worktree")
source_commit=$(current_commit "$worktree")

if [[ -n "$target_repo" ]]; then
  target_repo=$(resolve_worktree "$target_repo")
  target="$target_repo"
else
  target=$(main_worktree "$worktree")
fi

cd "$target"

git fetch origin "$base" >/dev/null 2>&1

git checkout "$base" >/dev/null 2>&1

git reset --hard "origin/$base" >/dev/null 2>&1

merge_target="$source_branch"
if [[ -n "$target_repo" ]]; then
  git fetch "$worktree" "$source_branch" >/dev/null 2>&1
  merge_target="FETCH_HEAD"
fi

merge_msg="Land ${source_branch} (${source_commit})"

set +e
git merge --no-ff -m "$merge_msg" "$merge_target" >/dev/null 2>&1
merge_status=$?
set -e

if [[ $merge_status -ne 0 ]]; then
  git merge --abort >/dev/null 2>&1 || true
  die "merge conflict"
fi

set +e
git push origin "$base" >/dev/null 2>&1
push_status=$?
set -e

if [[ $push_status -ne 0 ]]; then
  git reset --hard "origin/$base" >/dev/null 2>&1
  die "push failed"
fi

echo "Landed $(current_commit "$target")"

#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "$SCRIPT_DIR/common.sh"

worktree="."
base="master"
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
    --target-repo)
      target_repo="$2"
      shift 2
      ;;
    *)
      die "unknown argument: $1"
      ;;
  esac
done

worktree=$(resolve_worktree "$worktree")

if [[ -n "$target_repo" ]]; then
  target_repo=$(resolve_worktree "$target_repo")
  target="$target_repo"
else
  target=$(main_worktree "$worktree")
fi

land_commit=$(cd "$target" && git rev-parse "$base" 2>/dev/null || true)
if [[ -z "$land_commit" ]]; then
  echo "tutorial: could not resolve commit; skipping"
  exit 0
fi

set +e
cargo install --path "$target" --bin crank --force
install_status=$?
set -e

if [[ $install_status -ne 0 ]]; then
  echo "tutorial: cargo install failed (ignored)"
fi

set +e
crank tutorial generate --replace --worktree "$worktree" --base "$base" --merge-commit "$land_commit"
status=$?
set -e

if [[ $status -ne 0 ]]; then
  echo "tutorial: generation failed (ignored)"
fi

exit 0

#!/usr/bin/env bash
set -euo pipefail

die() {
  echo "$*" >&2
  exit 1
}

resolve_worktree() {
  local path="${1:-.}"
  (cd "$path" && pwd -P)
}

git_root() {
  local path="$1"
  (cd "$path" && git rev-parse --show-toplevel)
}

current_branch() {
  local path="$1"
  (cd "$path" && git rev-parse --abbrev-ref HEAD)
}

current_commit() {
  local path="$1"
  (cd "$path" && git rev-parse --short HEAD)
}

main_worktree() {
  local path="$1"
  local root
  root=$(git_root "$path")
  local main
  main=$(cd "$root" && git worktree list --porcelain | awk '/^worktree /{print $2; exit}')
  if [[ -z "$main" ]]; then
    main="$root"
  fi
  echo "$main"
}

#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "$SCRIPT_DIR/common.sh"

notify="false"
notify_interval="60000"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --notify)
      notify="true"
      shift 1
      ;;
    --notify-interval)
      notify_interval="$2"
      shift 2
      ;;
    --worktree|--base|--target-repo)
      shift 2
      ;;
    *)
      die "unknown argument: $1"
      ;;
  esac
done

if [[ "$notify" != "true" ]]; then
  exit 0
fi

if [[ "${CRANK_APPROVED:-}" == "1" ]]; then
  echo "Approval granted (CRANK_APPROVED=1)"
  exit 0
fi

if [[ -t 0 ]]; then
  read -r -p "Approve land? (y/N): " answer
  if [[ "$answer" == "y" || "$answer" == "Y" ]]; then
    echo "Approval granted"
    exit 0
  fi
  die "land rejected"
fi

die "approval required; rerun with CRANK_APPROVED=1"

#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  scripts/watch-run-logs.sh [RUN_ID|RUN_DIR]

Behavior:
  - No argument: watch the newest run under /tmp/crank-runs
  - RUN_ID:      watch /tmp/crank-runs/<RUN_ID>
  - RUN_DIR:     watch that explicit directory
EOF
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

base_dir="/tmp/crank-runs"
arg="${1:-}"

if [[ -z "$arg" ]]; then
  run_dir="$(ls -1dt "${base_dir}"/* 2>/dev/null | head -n1 || true)"
  if [[ -z "$run_dir" ]]; then
    echo "No runs found under ${base_dir}" >&2
    exit 1
  fi
elif [[ -d "$arg" ]]; then
  run_dir="$arg"
else
  run_dir="${base_dir}/${arg}"
fi

if [[ ! -d "$run_dir" ]]; then
  echo "Run dir not found: $run_dir" >&2
  exit 1
fi

mkdir -p "$run_dir/logs"
touch \
  "$run_dir/JOURNAL.md" \
  "$run_dir/logs/orchestrator.turns.log" \
  "$run_dir/logs/orchestrator.events.jsonl"

echo "Watching run logs in: $run_dir"
echo "Press Ctrl-C to stop."

exec tail -F \
  "$run_dir/JOURNAL.md" \
  "$run_dir/logs/orchestrator.turns.log" \
  "$run_dir/logs/orchestrator.events.jsonl"

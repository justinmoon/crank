#!/usr/bin/env bash
set -euo pipefail

payload="$(cat)"
project_dir="${CLAUDE_PROJECT_DIR:-}"
if [[ -z "$project_dir" ]]; then
  project_dir="$(printf '%s' "$payload" | awk -F'"cwd"' '{print $2}' | head -n1 | sed 's/^ *: *"//; s/".*$//')"
fi

if [[ -z "$project_dir" ]]; then
  exit 0
fi

marker="$project_dir/.crank/.current"
if [[ ! -f "$marker" ]]; then
  exit 0
fi

task_id="$(tr '\n' ' ' < "$marker" | tr ',' ' ' | tr -s ' ' | sed 's/^ //; s/ $//' | awk '{print $1}')"
if [[ -z "$task_id" ]]; then
  exit 0
fi

merged="$HOME/.crank/merged/$task_id"
help="$HOME/.crank/help/$task_id.md"

if [[ -f "$merged" || -f "$help" ]]; then
  printf '{"decision":"approve","reason":"merge/help marker present"}\n'
  exit 0
fi

printf '{"decision":"block","reason":"Continue until crank merge succeeds or crank ask-for-help."}\n'

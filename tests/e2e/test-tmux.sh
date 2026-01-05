#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=mux-harness.sh
source "$SCRIPT_DIR/mux-harness.sh"

run_mux_test "tmux" 2

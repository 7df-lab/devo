#!/usr/bin/env bash
# Compatibility entry point; assertions and cleanup are shared across platforms.
set -euo pipefail
exec node "$(dirname "$0")/run-tmux-smoke.js" "${1:-T1}"

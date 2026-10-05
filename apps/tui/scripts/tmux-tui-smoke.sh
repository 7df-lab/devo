#!/usr/bin/env bash
# Minimal InteractiveMode smoke under tmux (Unix).
# Usage: scripts/tmux-tui-smoke.sh [T1|T2|T3|goal|bash|agents|schedule|heartbeat]
# The heartbeat scenario needs tmux >= 3.2 (new-session -e).
set -euo pipefail
SCENARIO="${1:-T1}"
SESSION="${DEVO_TUI_TEST_SESSION:-devo-tui-test}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REPO="$(cd "$ROOT/../.." && pwd)"
DEVO="${DEVO_BIN:-${DEVO_SERVER_BIN:-$REPO/target/debug/devo}}"
TMUX_BIN="${DEVO_TMUX_BIN:-tmux}"

cleanup() { "$TMUX_BIN" kill-session -t "$SESSION" 2>/dev/null || true; }
trap cleanup EXIT

cleanup
"$TMUX_BIN" new-session -d -s "$SESSION" -c "$REPO" -x 100 -y 30 -- "$DEVO"
sleep 4
echo "=== boot ==="
"$TMUX_BIN" capture-pane -t "$SESSION" -p || true

case "$SCENARIO" in
  T1)
    echo "T1 ok (boot captured)"
    ;;
  T2)
    "$TMUX_BIN" send-keys -t "$SESSION" "Say hello in one short sentence." Enter
    sleep 25
    "$TMUX_BIN" capture-pane -t "$SESSION" -p || true
    echo "T2 ok (prompt sent)"
    ;;
  T3)
    "$TMUX_BIN" send-keys -t "$SESSION" "Write a long reply." Enter
    sleep 3
    "$TMUX_BIN" send-keys -t "$SESSION" Escape
    sleep 2
    echo "T3 ok (interrupt attempted)"
    ;;
  goal)
    "$TMUX_BIN" send-keys -t "$SESSION" "/goal" Enter
    sleep 2
    "$TMUX_BIN" send-keys -t "$SESSION" "/goal smoke objective" Enter
    sleep 3
    "$TMUX_BIN" capture-pane -t "$SESSION" -p || true
    echo "goal ok (slash sent)"
    ;;
  bash)
    "$TMUX_BIN" send-keys -t "$SESSION" "!echo smoke-bash" Enter
    sleep 4
    "$TMUX_BIN" capture-pane -t "$SESSION" -p || true
    echo "bash ok (command sent)"
    ;;
  agents)
    "$TMUX_BIN" send-keys -t "$SESSION" Escape
    sleep 1
    "$TMUX_BIN" send-keys -t "$SESSION" C-o
    sleep 2
    "$TMUX_BIN" capture-pane -t "$SESSION" -p || true
    echo "agents ok (view toggle attempted)"
    ;;
  schedule)
    "$TMUX_BIN" send-keys -t "$SESSION" "/heartbeat" Enter
    sleep 2
    "$TMUX_BIN" capture-pane -t "$SESSION" -p || true
    echo "schedule ok (heartbeat slash sent)"
    ;;
  heartbeat)
    # Full fire-and-deliver e2e under a real TTY. Reboot the session under an
    # isolated DEVO_HOME (tmux >= 3.2 new-session -e): a firing 1s heartbeat
    # writes the process-shared schedules.json, which must never be the user's.
    # settings.json marks onboarding shown so a fresh home boots into chat.
    HB_HOME="$(mktemp -d "${TMPDIR:-/tmp}/devo-tui-heartbeat-XXXXXX")"
    printf '{"onboardingShown":true}\n' > "$HB_HOME/settings.json"
    "$TMUX_BIN" kill-session -t "$SESSION" 2>/dev/null || true
    "$TMUX_BIN" new-session -d -s "$SESSION" -c "$REPO" -x 100 -y 30 \
      -e "DEVO_HOME=$HB_HOME" -- "$DEVO"
    sleep 5
    PROBE="hb-tui-fire-$(date +%s)"
    "$TMUX_BIN" send-keys -t "$SESSION" "/heartbeat every 1s $PROBE" Enter
    # The server schedule wake loop ticks every 15s; poll for the delivered
    # heartbeat prompt instead of a fixed sleep so slow CI cannot flake.
    FIRED=0
    DEADLINE=$(( $(date +%s) + 60 ))
    while [ "$(date +%s)" -lt "$DEADLINE" ]; do
      if "$TMUX_BIN" capture-pane -t "$SESSION" -p -S -300 2>/dev/null \
        | grep -q "$PROBE"; then
        FIRED=1
        break
      fi
      sleep 5
    done
    "$TMUX_BIN" capture-pane -t "$SESSION" -p -S -300 || true
    if [ "$FIRED" -ne 1 ]; then
      rm -rf "$HB_HOME"
      echo "FAIL: heartbeat fire not visible in pane (probe=$PROBE)" >&2
      exit 1
    fi
    echo "heartbeat fired and delivered ($PROBE)"
    "$TMUX_BIN" send-keys -t "$SESSION" "/heartbeat clear" Enter
    sleep 3
    CLEARED="$("$TMUX_BIN" capture-pane -t "$SESSION" -p -S -300 2>/dev/null || true)"
    echo "$CLEARED"
    rm -rf "$HB_HOME"
    if ! printf '%s' "$CLEARED" | grep -q "Heartbeat cleared"; then
      echo "FAIL: heartbeat clear not confirmed" >&2
      exit 1
    fi
    echo "heartbeat ok (set, fired, cleared)"
    ;;
  *)
    echo "boot ok ($SCENARIO)"
    ;;
esac

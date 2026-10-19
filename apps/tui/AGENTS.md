## Source of truth

Never read or edit `dist/**/*.js`. Those files are build output from `*.ts`; change TypeScript sources only.

## Testing Agent Interactive Mode with tmux

To test an agent's TUI in a controlled terminal environment:

```bash
# Create tmux session with specific dimensions
tmux new-session -d -s tui-test -x 80 -y 24

# Start Agent
tmux send-keys -t tui-test ".\target\debug\devo" Enter

# Wait for startup, then capture output
sleep 3 && tmux capture-pane -t tui-test -p

# Send input
tmux send-keys -t tui-test "your prompt here" Enter

# Send special keys
tmux send-keys -t tui-test Escape
tmux send-keys -t tui-test C-o  # ctrl+o
# Alt+Enter (queue follow-up). On Windows psmux, do NOT use Escape then Enter —
# Escape interrupts the active turn. Prefer a real Alt+Enter binding, or queue
# via the connection API / Ctrl chord if psmux cannot send Alt+Enter.
# (Historical note: Escape Enter was incorrectly documented as Alt+Enter.)

# Cleanup
tmux kill-session -t tui-test
```

You, yourself, are often running into a tmux session, so be careful when killing tmux sessions. Lots of other processes can be running on different tmux sessions/

---
artifact_id: L3-BEH-TUI-003
revision: 1
status: Draft
active_baseline: no
---

# L3-BEH-TUI-003 — Terminal Lifecycle Safety and Cleanup

## Purpose

Define the concrete behavior for terminal raw mode entry/exit, signal handling, crash cleanup, alternate screen management, and terminal state restoration to prevent broken terminals after agent exit.

## Source Design

L2-DES-TUI-005 (Terminal Lifecycle Safety)

## Behavior Specification

### B1. Terminal Raw Mode Entry

- **Trigger**: TUI client starts and connects to server.
- **Preconditions**: The process has a controlling terminal (stdin is a TTY). Crossterm backend is available.
- **Algorithm / Flow**:
  1. Before entering raw mode: save the current terminal state via `crossterm::terminal::enable_raw_mode()`.
  2. Enter alternate screen via `crossterm::execute!(stdout, EnterAlternateScreen)`.
  3. Enable mouse capture (if configured).
  4. Hide the cursor.
  5. If any step fails (e.g., stdout is not a TTY), fall back to line-mode interaction and log the reason.
- **Postconditions**: Terminal is in raw mode with alternate screen active. Original screen content is preserved.

### B2. Terminal Raw Mode Exit

- **Trigger**: TUI client exits normally.
- **Preconditions**: Raw mode and alternate screen were entered.
- **Algorithm / Flow**:
  1. Disable mouse capture.
  2. Show the cursor.
  3. Exit alternate screen via `crossterm::execute!(stdout, LeaveAlternateScreen)`.
  4. Disable raw mode via `crossterm::terminal::disable_raw_mode()`.
  5. All steps must be executed regardless of individual failures (use a drop guard or `std::panic::catch_unwind`).
- **Postconditions**: Terminal is restored to its pre-TUI state. Original screen content is visible.

### B3. Panic and Crash Cleanup

- **Trigger**: A Rust panic occurs in the TUI rendering loop or event handler.
- **Preconditions**: The TUI may be in an inconsistent state (raw mode active, screen partially rendered).
- **Algorithm / Flow**:
  1. A panic hook is registered at TUI startup.
  2. On panic: the hook runs BEFORE the default panic handler.
     a. Force-disable raw mode (crossterm's `disable_raw_mode` is safe to call even if not in raw mode).
     b. Force-leave alternate screen.
     c. Show cursor.
     d. Print the panic message and backtrace to stderr (which is now in cooked mode, so it displays correctly).
  3. If the hook itself panics: the OS terminal settings may be left in raw mode. The user may need to run `reset` to recover. Log a message explaining this.
- **Postconditions**: Terminal is almost always restored. In double-panic scenarios, the user is informed how to recover.

### B4. Signal Handling (SIGINT, SIGTERM, SIGWINCH)

- **Trigger**: Process receives a Unix signal.
- **Preconditions**: Signal handlers are registered via `tokio::signal`.
- **Algorithm / Flow**:
  1. **SIGINT** (Ctrl+C):
     a. First press: send `turn/interrupt` for the active turn (if any). Do NOT exit.
     b. Second press within 2 seconds: force quit. Exit the TUI with cleanup.
  2. **SIGTERM**: initiate graceful shutdown: disconnect from server, restore terminal, exit with code 0.
  3. **SIGWINCH** (terminal resize): trigger a re-render with new dimensions. Do not exit. See L3-BEH-TUI-001 B2.
  4. **SIGTSTP** (Ctrl+Z, suspend): restore terminal to cooked mode, send SIGSTOP to self. On SIGCONT, re-enter raw mode and re-render.
- **Postconditions**: Terminal state is consistent after each signal.

### B5. Alternate Screen Management

- **Trigger**: User presses `Ctrl+T` to view full transcript, or exits full transcript view.
- **Preconditions**: TUI is in the main alternate screen.
- **Algorithm / Flow**:
  1. On `Ctrl+T` press: the TUI opens a SECOND alternate screen (or repaints the current one in full-transcript mode). This is a modal overlay within the same alternate screen, not a nested alternate screen.
  2. On exit (second `Ctrl+T` or `Esc`): return to the live transcript view.
  3. On TUI exit while in full-transcript mode: the single alternate screen exit (B2) suffices — no nested alternate screens to unwind.
- **Postconditions**: The user never sees raw ANSI escape codes. The main terminal screen is restored on exit.

### B6. Terminal State Persistence Across TUI Restarts

- **Trigger**: TUI exits and is relaunched (e.g., server restart, user quits and reopens).
- **Preconditions**: The terminal is in cooked mode from the previous exit.
- **Algorithm / Flow**:
  1. On TUI start: check if the terminal is already in raw mode (defensive check). If yes, exit raw mode first, then re-enter cleanly.
  2. Persist no TUI-specific state on disk that would conflict with a fresh start.
  3. Alternate screen is always entered fresh — never attempt to resume a previous alternate screen session.
- **Postconditions**: Each TUI start is independent and clean.

## Traceability

| L2 Source | Relationship |
|---|---|
| L2-DES-TUI-005 | specified-by |

## Implementation Notes

- Use `crossterm` crate for terminal manipulation. Its `disable_raw_mode()` is safe to call unconditionally.
- Panic hook: register with `std::panic::set_hook`. The previous hook (default) is called after the custom hook.
- A Drop guard on the main TUI struct is the most reliable way to ensure cleanup. Even if `catch_unwind` fails, Drop runs.
- Alternate screen: use exactly one `EnterAlternateScreen`/`LeaveAlternateScreen` pair. Nested alternate screens are not supported by all terminals.

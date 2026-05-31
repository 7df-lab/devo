---
artifact_id: L3-BEH-TUI-001
revision: 2
status: Draft
active_baseline: no
---

# L3-BEH-TUI-001 — TUI Layout, Composer, and Input Modes

## Purpose

Define the concrete behavior for the TUI shell layout regions, composer text entry with submission semantics, session-local input modes, command prefixes, and responsive resize handling.

## Source Design

L2-DES-TUI-002 (Modern TUI Shell Layout), L2-DES-TUI-003 (Composer And Input Modes), L2-DES-TUI-005 (Terminal Lifecycle Safety), L2-DES-TUI-008 (TUI Style System)

## Behavior Specification

### B1. Shell Layout Regions

- **Trigger**: TUI starts and connects to a server.
- **Preconditions**: Terminal dimensions are known. Ratatui backend is initialized.
- **Algorithm / Flow**:
  1. Divide the terminal into vertical regions (top to bottom):
     - **Startup/header surface**: optional startup identity box from `L2-DES-TUI-002`. It is not a persistent one-row top bar.
     - **Transcript area** (flex): scrollable conversation history rendered from `TranscriptProjection`. Rendered cells include user messages, assistant text/reasoning, tool calls/results, approval prompts, errors, plan updates, compaction notices, and completed-turn summaries.
     - **Working indicator row** (0 or 1 row): live-only spinner label such as `⠋ Working · ⏱ 12s` while a turn is active.
     - **Composer band** (variable height): one padding row, one or more content rows, one padding row. The band uses `surface.inputBand`.
     - **Bottom status line** (1 row): mode, model display name, reasoning, token/cache counters, and context usage.
  2. Layout is computed on each render tick (frame-based, Ratatui).
  3. The transcript area scrolls independently from the bottom pane.
- **Postconditions**: The terminal is divided into stable regions. Each region updates independently.

### B2. Responsive Resize

- **Trigger**: Terminal window is resized (SIGWINCH on Unix).
- **Preconditions**: The TUI is rendering.
- **Algorithm / Flow**:
  1. Ratatui backend detects the resize and provides new `Rect`.
  2. Recompute layout: transcript area gets the new flex space. Composer keeps one padding row above and below the content and grows within configured bounds.
  3. Re-wrap transcript text at the new width. Store wrapped lines in the transcript buffer.
  4. If the composer is taller than available space (tiny terminal), cap at 1 row.
  5. Render the frame with the new layout.
- **Postconditions**: All regions are visible and text is re-wrapped. No visual glitches or overlapping.
- **Edge Cases**: Terminal shrinks to < 10 rows -> compact mode: startup/header surface is hidden, transcript area keeps at least 3 rows where possible, composer keeps 1 content row plus any available padding, and the bottom status line remains visible when space permits.

### B3. Composer Text Entry

- **Trigger**: User types in the composer area.
- **Preconditions**: The composer has focus. No modal overlay is active.
- **Algorithm / Flow**:
  1. On key press:
     - Printable characters (including Unicode/IME): insert at cursor.
     - `Enter`: submit unless the active terminal keybinding maps the key event to newline insertion.
     - Configured modified-enter fallback: insert newline without submitting.
     - `Ctrl+D` on empty line: send EOF / exit.
     - `Backspace`/`Delete`: remove character before/at cursor.
     - Arrow keys: navigate cursor within text.
     - `Ctrl+W`: delete word before cursor.
     - `Ctrl+U`: delete from cursor to start of line.
  2. Cursor position is tracked as (row, col) in the composer text buffer.
  3. The composer auto-grows: 1 row default, expands to 2 rows if text wraps, expands to 3 rows max. Beyond 3 rows, the text scrolls internally.
- **Postconditions**: Text is displayed in the composer. Cursor is visible. Multi-line content is supported.

### B4. Input Submission and Mode Handling

- **Trigger**: User presses `Enter` in the composer (not multi-line insert).
- **Preconditions**: Composer text is non-empty (or empty submission is configured to send).
- **Algorithm / Flow**:
  1. Read the current session-local input mode:
     - **Default Mode** (`Build`): Normal agent input. Text is submitted as `turn/submit` with `submission_mode: Normal`.
     - **Shell Mode** (`Shell`): Text is routed through the program's terminal command capability.
     - **Plan Mode** (`Plan`): Client marks the submission so the server applies Plan Mode rules.
  2. Detect command prefixes:
     - Leading `!` → Shell Mode for this submission (even in Default Mode).
     - Leading `/` → slash command (e.g., `/compact`, `/goal`, `/model`, `/permissions`). Handled by the TUI command registry and server RPC where needed.
     - Leading `@` → open fuzzy search for mentions. The text immediately after `@` is the query; no type prefix is required.
  3. If input mode is Shell Mode or starts with `!`: strip the `!` prefix (if present), submit as a command execution turn.
  4. If slash command: route to the slash command handler. Commands must come from the approved catalog in `L2-DES-TUI-003`.
  5. Clear composer after submission. The initiating client may optimistically render input, but must reconcile against canonical server events.
- **Postconditions**: The user's intent is submitted to the server. The composer is cleared and ready for the next input.
- **Edge Cases**: Empty submission → do nothing (beep/visual feedback). Only whitespace → treat as empty. Leading `!` with no command → treat as Shell Mode with empty command (server rejects).

### B5. Session-Local Input Mode Switching

- **Trigger**: User invokes a mode switch: pressing `!` at the start of empty composer, choosing a mode control, or using a dedicated keybinding.
- **Preconditions**: Composer is focused.
- **Algorithm / Flow**:
  1. **Shell Mode activation**: when composer is empty and user types `!` as first character, the client SHOWS a "Shell" mode indicator in the status line. The `!` remains in the composer.
  2. **Plan Mode activation**: use the approved mode control or keybinding. Do not add `/plan` to the slash-command catalog unless the L2 command catalog is explicitly revised.
  3. **Mode display**: Bottom status line shows current submission mode label: `Build`, `Plan`, or `Shell`.
  4. Shell Mode resets to Default after command completion unless a later approved design adds persistent Shell Mode.
  5. Plan Mode state follows the session-local interaction-mode rules. It is visible in the status line and enforced by server-side tool policy.
- **Postconditions**: The user's next submission uses the selected mode. Status line reflects the mode.

### B6. Bottom Status Line

- **Trigger**: Every render frame.
- **Preconditions**: TUI is rendering the bottom pane.
- **Algorithm / Flow**:
  1. Status line (1 row) displays the L2-defined fields:
     - mode label (`Build`, `Plan`, or `Shell`),
     - current model display name, falling back to model slug only when display name is unavailable,
     - current reasoning effort,
     - input token count and cache details as `↑N[cached M P%]`,
     - output token count as `↓N`,
     - context usage bar, percentage, current usage, and effective context limit.
  2. Colors: mode label uses `mode.build`, `mode.plan`, or `mode.shell`.
  3. Active-work spinners are rendered in the working indicator row, not by replacing the status line fields.
- **Postconditions**: User can see current mode, context pressure, and available shortcuts at a glance.

## Traceability

| L2 Source | Relationship |
|---|---|
| L2-DES-TUI-002 | specified-by |
| L2-DES-TUI-003 | specified-by |
| L2-DES-TUI-005 | specified-by |
| L2-DES-TUI-008 | specified-by |

## Implementation Notes

- Built with `ratatui` crate. Layout uses `Constraint::Length` for fixed rows and `Constraint::Min`/`Percentage` for flex.
- Composer uses `tui-textarea` crate for multi-line text editing with cursor support.
- Terminal resize is handled by `tokio::signal::unix::SignalKind::window_change()` on Unix, or crossterm's resize event.

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-27 | Assistant | Initial | Initial layout, composer, and input mode behavior. |
| 2 | 2026-05-27 | Assistant | Correction | Aligned region layout, status line, `@` query behavior, Plan Mode controls, and active-work spinner placement with L2 TUI designs. |

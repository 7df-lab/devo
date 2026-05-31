---
artifact_id: L3-BEH-TUI-002
revision: 2
status: Draft
active_baseline: no
---

# L3-BEH-TUI-002 — Streaming Transcript Rendering and State Visibility

## Purpose

Define the concrete behavior for rendering live streaming content, tool call progress, approval modals, plan/goal state updates, compaction notices, and completed transcript review.

## Source Design

L2-DES-TUI-004 (Streaming Transcript And State), L2-DES-TUI-002 (Modern TUI Shell Layout), L2-DES-TUI-007 (Session Rendering Consistency), L2-DES-TUI-008 (TUI Style System), L2-DES-APP-003 (Client Server Protocol)

## Behavior Specification

### B1. Live Streaming Content Rendering

- **Trigger**: `item_started` and `item_content_update` events arrive from the server.
- **Preconditions**: The TUI is subscribed to the session. The transcript area is rendered.
- **Algorithm / Flow**:
  1. On `item_started`: create a new transcript cell for the item with `kind`, `role`, initial content (if any), and `created_at`.
  2. On `item_content_update`:
     a. Find the target item by `item_id`.
     b. Append or replace content at the specified `content_part_index`.
     c. Parse content for ANSI escape codes (from command output) and render with terminal-safe color stripping or conversion.
     d. Wrap text at current transcript width.
     e. Auto-scroll: if the user is at the bottom of the transcript (last 3 lines visible), auto-scroll to keep the latest content visible. If the user has scrolled up, do NOT auto-scroll — show a "↓ New content" indicator.
  3. On `item_completed`: show the item's `final_status`. For successful items, render as final. For failed items, render with error styling (red, `✗` prefix).
  4. Streaming text for assistant items uses a dim cursor/typing indicator until the item completes.
- **Postconditions**: The transcript shows live progress. User can scroll back during streaming.

### B2. Tool Call Rendering

- **Trigger**: `tool_call_started`, `tool_call_updated`, `tool_call_completed` events.
- **Preconditions**: The transcript is rendering.
- **Algorithm / Flow**:
  1. On `tool_call_started`: render a tool call cell using the `L2-DES-TUI-004` transcript grammar:
     - All tool cells begin with a single `┃` marker.
     - Read, glob, and grep are grouped visually under `Explore`, with one call per line.
     - Write renders as `Create <path>` followed by diff content.
     - Apply patch renders as `Edit <path>` followed by diff content.
     - Shell renders as `Running <command>` while active and `Run <command>` when completed.
     - `command_description` may be shown for command-like tools as a concise intent label.
  2. On `tool_call_updated`: update progress. Show `output_preview` (truncated, redacted) if available.
  3. On `tool_call_completed`: replace active state with the terminal text state (`passed`, `failed`, `denied`, `interrupted`, or `canceled`) and the elapsed time.
     - Show `result_summary` (natural-language, max 2 lines).
     - Show structured status inline (exit code, HTTP status, file count).
     - Collapse detailed output behind an expandable section.
  4. For `multi_tool_use` parent: render as a group container with child count and group status. Children render indented under the parent.
- **Postconditions**: Tool activity is clearly visible. Results are concise but auditable.

### B3. Approval Modal Overlay

- **Trigger**: `approval/requested` event.
- **Preconditions**: The TUI is connected and subscribed.
- **Algorithm / Flow**:
  1. Render a modal overlay on top of the transcript area (lower half of screen).
  2. Display:
     - ⚠ "Approval Required" header.
     - Tool/action summary.
     - Resource details (path, host, or command).
     - Agent's justification (if provided).
     - Available scopes: `[y] Approve Once  [s] Approve for Session  [n] Deny  [Esc] Cancel`.
  3. Keyboard input is captured by the modal (composer is blocked).
  4. On user keypress:
     - `y` → send `approval/respond` with `decision: Allow`, `scope: Once`.
     - `s` → send with `decision: Allow`, `scope: Session`.
     - `n` → send with `decision: Deny`.
     - `Esc` → send with `decision: Deny` (or cancel for MCP elicitation).
  5. Multiple pending approvals are queued. After resolving one, the next appears.
  6. On `approval_resolved`: dismiss the modal. Show a brief resolution notice in the transcript.
- **Postconditions**: User's decision is sent to server. Tool call proceeds or is denied.

### B4. State Visibility Indicators

- **Trigger**: Turn status changes or specific state events arrive.
- **Preconditions**: The TUI is rendering.
- **Algorithm / Flow**: The TUI shows distinct visual states:
  - **Idle**: steady cursor in composer, no spinner, current model and reasoning are visible in the bottom status line and startup/header projection where that header is currently shown.
  - **Generating**: working indicator row uses the standard spinner and elapsed-time format, for example `⠋ Working · ⏱ 12s`.
  - **Tool Running**: tool cell shows running state; the working indicator remains between transcript and composer.
  - **Waiting for Approval**: modal overlay, status line shows "Waiting for approval".
  - **Waiting for Question**: modal overlay (simpler than approval), status: "Question".
  - **Interrupted**: turn completed with interrupted marker (⏸), status returns to Idle.
  - **Failed**: turn completed with error marker (✗), error details shown inline.
  - **Background Process**: process cell in transcript shows runtime counter, stop button hint.
- **Postconditions**: User always knows what the agent is doing.

### B5. Plan and Goal State Rendering

- **Trigger**: `plan_updated` or `goal_updated` events.
- **Preconditions**: The TUI is subscribed.
- **Algorithm / Flow**:
  1. **Plan updates**: render a plan cell in the transcript with:
     - Objective line.
     - Item list with status symbols from `L2-DES-TUI-008`: `○` pending, `●` in progress, `✓` completed, `⚠` blocked.
     - Parent/child indentation for hierarchical items.
     - `parallel_group_id` shown as bracketed label.
  2. **Goal updates**: show as a transcript cell, goal panel, or status-line adjunct depending on available space:
     - Objective preview (truncated to 60 chars).
     - Budget progress: `[====     ] 40% tokens, 2/10 turns`.
     - Status: Active, Paused, Blocked, Complete, Canceled, Budget Limited.
- **Postconditions**: Task planning and goal progress are visible at a glance.

### B6. Compaction Notices

- **Trigger**: `context_updated` event with compaction status.
- **Preconditions**: The transcript is rendering.
- **Algorithm / Flow**:
  1. On compaction start: render a transcript-area status cell with label:
     - `Manual Compaction Started` (if trigger_source is manual).
     - `Automatically Compaction Started` (if trigger_source is automatic).
  2. On compaction done: render `Compaction Done` with summary: "N turns compacted, M turns preserved".
  3. These are status cells, not user/assistant messages. They are rendered in a dimmed, distinct style.
- **Postconditions**: The user knows compaction occurred and what was compacted.

### B7. Full Transcript Review (Ctrl+T)

- **Trigger**: User presses `Ctrl+T`.
- **Preconditions**: The TUI is in normal mode.
- **Algorithm / Flow**:
  1. Enter alternate screen. Render the FULL session transcript (all turns, not just the active context window).
  2. Show compacted turns in a collapsed section: "N turns compacted — press Enter to expand summary".
  3. Full transcript is scrollable (PgUp/PgDn, arrow keys, Home/End).
  4. Press `Ctrl+T` again or `Esc` to return to live view.
  5. Auto-scroll to the bottom (latest turn) on re-entry.
- **Postconditions**: User can review complete session history, including compacted turns.

## Traceability

| L2 Source | Relationship |
|---|---|
| L2-DES-TUI-004 | specified-by |
| L2-DES-TUI-002 | specified-by |
| L2-DES-TUI-007 | specified-by |
| L2-DES-TUI-008 | specified-by |
| L2-DES-APP-003 | specified-by |

## Implementation Notes

- Transcript cells use a custom `Paragraph` widget with line wrapping at the current area width.
- Scroll state: track `scroll_offset` (lines from bottom). Auto-scroll when `scroll_offset <= 0`.
- ANSI escape code handling: use `strip_ansi_escapes` crate, or selectively render supported SGR codes (colors, bold, italic) via Ratatui spans.
- Avoid emoji as core state icons. Use the symbol system in `L2-DES-TUI-008` so terminal width remains predictable.

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-27 | Assistant | Initial | Initial streaming transcript behavior. |
| 2 | 2026-05-27 | Assistant | Correction | Replaced emoji/ad hoc tool rendering with the approved transcript grammar, spinner format, and style symbols. |

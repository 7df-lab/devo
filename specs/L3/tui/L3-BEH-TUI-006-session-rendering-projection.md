---
artifact_id: L3-BEH-TUI-006
revision: 1
status: Draft
active_baseline: no
---

# L3-BEH-TUI-006 — Session Rendering Projection

## Purpose

Define the concrete TUI projection pipeline that makes live server events and restored session history render through the same cell models and renderers.

## Source Design

L2-DES-TUI-007 (Session Rendering Consistency), L2-DES-APP-003 (Client Server Protocol), L2-DES-CONV-001 (Session JSONL Data Model), L2-DES-TUI-004 (Streaming Transcript and State)

## Behavior Specification

### B1. Projection Ownership

- **Trigger**: The TUI opens, subscribes to, resumes, or restores a session.
- **Preconditions**: The client has either server-client events, a `session_loaded` snapshot, or durable replay output.
- **Algorithm / Flow**:
  1. Normalize all inputs into a single `TranscriptProjection`.
  2. Store renderable cells as `TranscriptCellModel` values.
  3. Render only from `TranscriptProjection`; do not render directly from raw JSONL records or raw server notification payloads.
  4. Keep live-only widget state outside the projection: spinner frame, cursor position, viewport offset, popup focus, and composer buffer.
- **Postconditions**: Live and restored sessions use the same renderer path.

### B2. Projection Types

```rust
pub struct TranscriptProjection {
    pub session_id: SessionId,
    pub revision: u64,
    pub cells: Vec<TranscriptCellModel>,
    pub active_turn: Option<ActiveTurnProjection>,
    pub usage: Option<UsageProjection>,
    pub context: Option<ContextProjection>,
    pub diagnostics: Vec<DiagnosticProjection>,
}

pub struct TranscriptCellModel {
    pub cell_id: TranscriptCellId,
    pub cell_type: TranscriptCellType,
    pub turn_id: Option<TurnId>,
    pub item_refs: Vec<ItemId>,
    pub role: TranscriptRole,
    pub status: CellStatus,
    pub content_parts: Vec<RenderablePart>,
    pub mentions: Vec<Mention>,
    pub tool_metadata: Option<ToolCellMetadata>,
    pub result_summary: Option<String>,
    pub display_diff_ref: Option<DisplayDiffRef>,
    pub folding_state: FoldingState,
    pub timing: Option<CellTiming>,
    pub usage_summary: Option<TurnUsage>,
    pub provenance: ProjectionProvenance,
}
```

`cell_id` must be stable across live completion and restored replay for the same logical transcript cell so scrolling and golden-render tests remain deterministic.

### B3. Live Event Normalization

- **Trigger**: Concrete session-scoped or turn-scoped notifications arrive, such as `session/started`, `turn/started`, `item/started`, `item/agentMessage/delta`, approval, question, search, or config notifications.
- **Preconditions**: The client is subscribed and initialized.
- **Algorithm / Flow**:
  1. Apply events by increasing session sequence.
  2. If an event arrives out of order, buffer it briefly and request catch-up if the gap remains.
  3. Map event kinds to projection changes:
     - `turn_started` creates or updates `active_turn`.
     - `item_started` creates a cell.
     - `item_content_update` updates the existing cell content.
     - `tool_call_started/updated/completed` update tool cells.
     - `plan_updated` updates plan cells.
     - `usage_updated` updates usage projection and completed-turn summaries.
     - `context_updated` updates context pressure and compaction cells.
  4. Mark cells from live events as `provenance: Live`.
- **Postconditions**: The visible transcript changes through projection diffs.

### B4. Replay Normalization

- **Trigger**: The client receives `session_loaded`, opens a saved session, or enters full transcript review.
- **Preconditions**: The server supplied a session snapshot or replay projection.
- **Algorithm / Flow**:
  1. Convert replayed turns, items, tool results, usage, context, plan, and goal projections into the same `TranscriptCellModel` shapes used for live rendering.
  2. Mark cells from restored history as `provenance: Replay`.
  3. Represent missing display data explicitly with `status: Degraded` and diagnostics rather than switching renderer style.
  4. Full transcript review reuses the same projection and renderers with different viewport and folding policies.
- **Postconditions**: A completed live turn and the same restored turn render equivalently at the same width.

### B5. Renderer Contract

- **Trigger**: The render loop needs visible rows.
- **Preconditions**: A `TranscriptProjection` exists.
- **Algorithm / Flow**:
  1. Select a renderer by `TranscriptCellType`, not by live/replay provenance.
  2. Pass the same style token set from `L3-BEH-TUI-007` to every renderer.
  3. Recompute wrapped rows on terminal width change from cell models, never from previously drawn terminal rows.
  4. Render live-only overlays after durable cells: working indicator, cursor, selection, popup focus.
- **Postconditions**: Resize, restore, and live update behavior remain deterministic.

## Required Tests

- Applying a representative live event sequence and replaying equivalent durable records produce equivalent `TranscriptProjection` after completion.
- Equivalent projections render the same visible text at 80, 120, and 160 columns.
- Completed shell, read, grep, write, apply-patch, plan, approval, and error cells render through the same functions for live completion and restored replay.
- `Ctrl+T` uses the same projection source as inline transcript rendering.
- Resize recomputes rows from projection state and preserves scroll anchoring by `cell_id`.

## Traceability

| L2 Source | Relationship |
|---|---|
| L2-DES-TUI-007 | specified-by |
| L2-DES-TUI-004 | specified-by |
| L2-DES-APP-003 | specified-by |
| L2-DES-CONV-001 | specified-by |

## Implementation Notes

- Put projection state in the TUI crate, not in server business logic.
- Server snapshots should already be safe projections; the TUI still owns the final cell model used for rendering.
- Do not store animation frames or viewport offsets in durable session data.

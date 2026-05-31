---
artifact_id: L2-DES-TUI-007
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-25
---

# L2-DES-TUI-007 — Session Rendering Consistency

## Purpose

Refine session rendering consistency for the TUI by requiring live sessions and restored sessions to use the same transcript projection and cell-rendering pipeline.

## Background / Context

The TUI renders a session in two common situations:

- Live rendering while the server emits turn, item, tool, usage, and state events.
- Restored rendering after the program replays durable session records or opens a saved session snapshot.

If these paths render independently, restored history can drift from the live experience even when the underlying session items are equivalent. The TUI should instead normalize both paths into one shared projection before rendering cells.

## Source Requirements

- `L1-REQ-CLIENT-002` requires active sessions and restored session history to use a consistent visual and stylistic language.
- `L1-REQ-TUI-002` requires timely streaming of assistant, reasoning, tool, and state updates.
- `L1-REQ-TUI-003` requires a durable, readable, scrollable transcript.
- `L1-REQ-TUI-004` requires visible current execution state.
- `L1-REQ-TUI-007` requires stable responsive layout and resize behavior.
- `L1-REQ-CLIENT-001` requires Unicode-safe and localization-ready display behavior.
- `L2-DES-APP-003` defines server-client event payloads.
- `L2-DES-CONV-001` defines durable session events and replay state.
- `L2-DES-TUI-002` defines shell placement for the transcript viewport.
- `L2-DES-TUI-004` defines TUI transcript cells and live streaming state.
- `L2-DES-TUI-006` defines the full transcript alternate-screen review surface.

## Design Requirement

The TUI must render live sessions and restored sessions through the same normalized transcript projection.

```text
Server-client live events          Durable replay/session snapshot
           ↓                                  ↓
     Event normalizer                  Replay normalizer
           └──────────────┬───────────────┘
                          ↓
                TranscriptProjection
                          ↓
                  TUI cell renderers
                          ↓
                 Visible transcript frame
```

The renderer boundary is the critical rule: TUI cell renderers should consume `TranscriptProjection` state, not raw server events, raw JSONL records, or separate live/restored view models.

## Projection Model

`TranscriptProjection` is a conceptual client-side model of what the TUI can render for a session. It is derived from canonical server state and durable transcript state.

Representative projection shape:

```text
TranscriptProjection
  session_id
  revision
  cells: [TranscriptCellModel]
  active_turn: ActiveTurnProjection?
  usage: UsageProjection?
  context: ContextProjection?
  diagnostics: [DiagnosticProjection]
```

`TranscriptCellModel` should carry enough information for both live and restored rendering:

| Field | Purpose |
|---|---|
| `cell_id` | Stable renderer identity for diffing and scroll anchoring. |
| `cell_type` | User, assistant, reasoning, explore, file mutation, shell, approval, question, error, plan, context, or completed-turn summary. |
| `turn_id` | Turn association for grouping, edit targeting, and completed summary placement. |
| `item_refs` | References to durable transcript items that produced the cell. |
| `role` | User, assistant, tool, system-status, or client-status rendering role. |
| `status` | Streaming, running, waiting, completed, failed, interrupted, canceled, restored, degraded, or unknown. |
| `content_parts` | Text, Markdown, diff, image reference, output excerpt, or structured summary parts. |
| `mentions` | User-message references to files, skills, MCPs, images, or other supported mention types. |
| `tool_metadata` | Tool name, command, target path, arguments summary, approval state, and safety/redaction metadata. |
| `result_summary` | Bounded natural-language or structured result summary used for folded rendering. |
| `display_diff_ref` | Reference to persisted or reconstructed diff data for file mutation cells. |
| `folding_state` | Output fold counts, shown ranges, and full-transcript availability. |
| `timing` | Start time, end time, elapsed time, and duration display source. |
| `usage_summary` | Turn-level token and context usage when available. |
| `provenance` | Whether the cell currently comes from live events, durable replay, or a mixed live-plus-durable reconciliation. |

IDs in the design identify durable or logical references. Implementation may store direct references, handles, indexes, or typed object links as long as the replayed projection is deterministic and equivalent.

## Live And Replay Normalization

Live event normalization maps server-client notifications into projection updates:

| Event | Projection Effect |
|---|---|
| `session_loaded` | Replace or initialize the projection from a server-confirmed session snapshot. |
| `metadata_updated` | Update session header, active mode, model, reasoning, persona, permission, and workspace display data. |
| `turn_started` | Create or update an active turn projection and prepare turn-level grouping. |
| `turn_status_changed` | Update active turn status, waiting state, failure state, or completion state. |
| `item_started` | Create the corresponding cell model immediately. |
| `item_content_update` | Append or replace live content in the existing cell model. |
| `item_completed` | Mark the cell model complete and reconcile live overlay content into durable review content. |
| `item_failed` | Mark the cell failed and attach diagnostic or recovery metadata. |
| `tool_call_started` | Create a tool cell with command, target, and pending status. |
| `tool_call_updated` | Update tool progress, streaming output excerpt, or folded line counts. |
| `tool_call_completed` | Convert running tool state into completed, failed, canceled, or interrupted result cells. |
| `usage_updated` | Update usage projection and any affected completed-turn summary. |
| `context_updated` | Update context pressure and emit context or compaction status cells where durable. |
| `error_reported` | Create or update an error cell with phase, recoverability, and recovery action. |
| `plan_updated` | Create or update visible plan cells when plan state is part of transcript display. |

Replay normalization maps durable session records into the same projection shape. It must not render directly from raw JSONL lines. Durable records should produce the same `TranscriptCellModel` fields that live events produce after completion.

## Rendering Rules

- Live and restored paths must converge before terminal rendering.
- Completed live cells and restored cells of the same kind must call the same renderer with equivalent cell models.
- The TUI must not maintain separate renderers named by source, such as one renderer for live assistant cells and another renderer for replayed assistant cells.
- Raw durable JSONL records may be inspected for debugging, but normal transcript display must render from projection state.
- Live-only overlays such as the spinner, cursor, composer text, and animation frame may remain separate transient inputs.
- Live-only overlays must reconcile into durable cell models when the server finalizes the related turn or item.
- Missing metadata must be represented explicitly with `unknown` or `degraded` state rather than silently switching to a different visual format.
- Terminal resize must rebuild visible rows from projection state, not from previously drawn terminal rows.
- The inline transcript and the `Ctrl+T` full transcript overlay should use the same projection source and cell renderers, with only placement, viewport height, selection state, and folding limits differing.

## State Conversion Examples

Streaming assistant response:

```text
Live events:
  item_started assistant_response
  item_content_update "The parser accepts quoted values..."
  item_completed

Projection:
  AssistantMessageCell(status=streaming)
  AssistantMessageCell(status=completed)

Renderer:
  same assistant message renderer in both states
```

Restored assistant response:

```text
Durable replay:
  assistant response item with final content

Projection:
  AssistantMessageCell(status=completed)

Renderer:
  same assistant message renderer used by the completed live cell
```

Shell command:

```text
Live:
┃ Running  cargo test parser::quoted -- --nocapture

Completed and restored:
┃ Run      cargo test parser::quoted -- --nocapture       failed  2.3s
  ┗ output  64 lines hidden, 12 shown                     Ctrl+T for full transcript
      test parser::quoted_escape ... FAILED
```

The running cell is live-only. The completed `Run` cell is durable review content and must render the same after replay.

Compaction status:

```text
Live event or durable replay:
┃ Manual Compaction Started
┃ Compaction Done
```

Compaction cells are transcript-area status cells in both paths.

## Live Versus Restored TUI Example

Live while work is running:

```text
┃ Fix the parser regression and run the focused tests.

┃ Thought: The failure is isolated to escaped quote handling.

┃ Explore
  ┗ Read  crates/parser/src/lib.rs
    Grep  "quoted_escape" tests

┃ Running  cargo test parser::quoted -- --nocapture

⠋ Working · ⏱ 12s
```

Restored after completion:

```text
┃ Fix the parser regression and run the focused tests.

┃ Thought: The failure is isolated to escaped quote handling.

┃ Explore
  ┗ Read  crates/parser/src/lib.rs
    Grep  "quoted_escape" tests

┃ Run      cargo test parser::quoted -- --nocapture       passed  2.1s
  ┗ output  18 lines hidden, 8 shown                      Ctrl+T for full transcript

┃ The focused parser tests now pass.

  ▣ Build · DeepSeek V4 Pro · 2.1s
```

The restored view omits the live working spinner because it is transient, but all durable cells use the same visual grammar and renderer path as their completed live equivalents.

## Persistence Expectations

The durable session model must preserve enough display-relevant data for projection reconstruction:

- Item role, kind, status, ordering, and turn association.
- User content parts and mentions.
- Assistant text and visible reasoning summaries where emitted.
- Tool names, arguments summaries, targets, statuses, timing, result summaries, and output fold metadata.
- File mutation diff references or equivalent persisted diff material.
- Approval and question prompt text, state, and resolution.
- Error phase, recoverability, and recovery action where available.
- Usage and context summaries that affect visible completed-turn metadata.
- Compaction lifecycle records that render as transcript status cells.

The TUI should not require persisting animation frames, cursor positions, transient scroll offsets, local hover/selection state, or implementation-only widget state to make restored transcript history consistent.

## Testing Strategy

The TUI should include projection and rendering tests that compare live and replay paths:

- Projection equivalence: applying a representative live event sequence and replaying the equivalent durable records should produce equivalent `TranscriptProjection` state after completion.
- Golden rendering: equivalent projections should render the same visible cell text and layout at the same terminal width.
- Resize rebuild: after terminal width changes, visible rows should be recomputed from projection state.
- Tool replay: completed read, grep, shell, write, and apply-patch cells should remain recognizable and use the same cell renderers after restoration.
- Failure replay: failed, interrupted, canceled, approval, question, and error cells should preserve state distinctions after restoration.
- Full transcript overlay: `Ctrl+T` should render from the same projection as the inline transcript, with only viewport and folding policy differences.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-CLIENT-002 | 1 | specs/L1/L1-REQ-CLIENT-002-session-rendering-consistency.md | Defines the TUI projection and renderer boundary that makes live and restored session rendering consistent. |
| related-to | L1-REQ-TUI-002 | 1 | specs/L1/L1-REQ-TUI-002-streaming.md | Live streaming events must normalize into the shared projection. |
| related-to | L1-REQ-TUI-003 | 1 | specs/L1/L1-REQ-TUI-003-transcript.md | Transcript cells are rendered from the shared projection. |
| related-to | L1-REQ-TUI-004 | 1 | specs/L1/L1-REQ-TUI-004-state-visibility.md | State visibility depends on explicit projected cell states. |
| related-to | L1-REQ-TUI-007 | 1 | specs/L1/L1-REQ-TUI-007-responsive-layout-readability.md | Resize must rebuild visible rows from projection state. |
| related-to | L1-REQ-CLIENT-001 | 1 | specs/L1/L1-REQ-CLIENT-001-localization-readiness.md | Projection and rendering must preserve Unicode and display-width correctness. |
| related-to | L2-DES-APP-003 | 2 | specs/L2/app/L2-DES-APP-003-client-server-protocol.md | Server-client events feed the live normalization path. |
| related-to | L2-DES-CONV-001 | 1 | specs/L2/conv/L2-DES-CONV-001-session-jsonl-data-model.md | Durable session records feed the replay normalization path. |
| related-to | L2-DES-TUI-002 | 1 | specs/L2/tui/L2-DES-TUI-002-modern-tui-shell-layout.md | The shell places transcript projection output in the viewport. |
| related-to | L2-DES-TUI-004 | 1 | specs/L2/tui/L2-DES-TUI-004-streaming-transcript-and-state.md | Transcript cell grammar and live overlay behavior are rendered through the shared projection. |
| related-to | L2-DES-TUI-006 | 1 | specs/L2/tui/L2-DES-TUI-006-full-transcript-alternate-screen.md | The full transcript overlay should reuse the same projection and cell renderers. |
| specified-by | L3-BEH-TUI-006 | 1 | specs/L3/tui/L3-BEH-TUI-006-session-rendering-projection.md | L3 defines shared transcript projection, live/replay normalizers, renderer contract, and equivalence tests. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-25 | Assistant | Initial | Initial TUI session rendering consistency design with shared projection, live/replay normalization, renderer boundary, examples, and testing strategy. |

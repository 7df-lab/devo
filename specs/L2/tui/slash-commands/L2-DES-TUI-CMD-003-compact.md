---
artifact_id: L2-DES-TUI-CMD-003
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-25
---

# L2-DES-TUI-CMD-003 — Slash Command: /compact

## Purpose

Define the TUI behavior for `/compact`, which asks the server to compact the current session context while preserving the full transcript.

## Command Contract

- Command: `/compact`
- Description: `compact the current session context`
- Parameters: none in the first milestone.
- Mutability: active context snapshot and durable compaction records.
- Active-turn availability: blocked while a turn is actively generating or running tools.

## UI Flow

`/compact` asks for confirmation before starting compaction.

```text
┃ /compact
```

During compaction:

```text
┃ Manual Compaction Started

⠋ Working · 4s
```

After successful compaction:

```text
┃ Compaction Done
```

## State And Error Behavior

- The command must not delete transcript items.
- Starting manual compaction must add a transcript-area status cell with the exact text `Manual Compaction Started`.
- Successful compaction creates durable context summary state and updates the active context snapshot.
- The TUI should show `Compaction Done` in the transcript area when `context_updated` reports successful compaction completion.
- If compaction fails, the prior context snapshot remains active and the TUI shows an error with a recovery hint.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-TUI-006 | 1 | specs/L1/L1-REQ-TUI-006-command-discovery-control.md | Defines command-specific behavior for a discoverable TUI command. |
| related-to | L1-REQ-CONTEXT-003 | 1 | specs/L1/L1-REQ-CONTEXT-003-compress.md | Context compaction is the underlying workflow. |
| related-to | L2-DES-CONTEXT-002 | 1 | specs/L2/context/L2-DES-CONTEXT-002-context-compaction.md | Defines compaction triggers, summaries, and context updates. |
| related-to | L2-DES-TUI-004 | 1 | specs/L2/tui/L2-DES-TUI-004-streaming-transcript-and-state.md | Defines working indicator and context update display. |
| related-to | L2-DES-TUI-003 | 1 | specs/L2/tui/L2-DES-TUI-003-composer-and-input-modes.md | Uses shared slash-command discovery and invocation behavior. |
| specified-by | TBD | TBD | specs/L3/tui/TBD.md | L3 behavior has not been authored yet. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-23 | Assistant | Initial | Initial `/compact` command design. |
| 1 | 2026-05-25 | Human | Refinement | Added transcript-area notices for manual compaction start and completion. |

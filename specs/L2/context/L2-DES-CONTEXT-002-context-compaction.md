---
artifact_id: L2-DES-CONTEXT-002
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-25
---

# L2-DES-CONTEXT-002 — Context Compaction

## Purpose

Define how the program detects context pressure, selects eligible history for summarization, produces a durable compaction summary, and updates active context references so future model invocations use the compacted representation while the full transcript remains available for user review.

## Background / Context

`L2-DES-CONV-001` defines durable `context_compaction_started` and `context_compaction_completed` records, and states that compaction creates a summary record referenced by later context snapshots instead of older detailed transcript records. `L2-DES-AGENT-001` places compaction as an optional step before the primary model request when context pressure requires it. `L2-DES-CONTEXT-001` defines how metadata-derived content and change-signal messages compose into context, and notes that compacted change-signal messages from earlier turns may be summarized or omitted.

This document defines when compaction triggers, what eligible history looks like, what the summary must preserve, how the active context snapshot changes after compaction, and how non-compacted transcript records remain available for user review.

## Source Requirements

- `L1-REQ-CONTEXT-003` requires compaction when context usage reaches a configured threshold, with summaries preserving task continuity, recent turns remaining uncompressed, and raw history remaining recoverable.
- `L1-REQ-CONTEXT-001` requires useful model context management across long-running sessions.
- `L1-REQ-LLM-001` requires stable context prefixes and append-only handling of configuration changes.
- `L2-DES-CONV-001` defines durable compaction records, context snapshots, and summary item references.
- `L2-DES-AGENT-001` defines the execution engine phase where compaction occurs.
- `L2-DES-CONTEXT-001` defines the immutable prefix and metadata-derived content that must remain valid after compaction.
- `L2-DES-TUI-004` defines transcript-area rendering for compaction lifecycle notices.

## Design Requirement

The program should compact older transcript history into a summary when the estimated model-visible context approaches the effective context limit. Compaction should produce a durable summary record that preserves task continuity (objectives, decisions, changed files, blockers, verification status) while allowing the full raw transcript to remain available outside the model context. The active context snapshot should reference the summary instead of individual compacted turns for future invocations.

Compaction is append-only from the durable storage perspective. It creates new summary records and updated context snapshots. It does not delete, mutate, or rewrite existing transcript turns.

## Trigger Condition

Compaction should be considered before a model invocation when the estimated token count of the assembled context approaches the model's effective context limit.

Conceptual trigger fields:

- `current_token_estimate`: the token estimate for the current context snapshot before the pending invocation.
- `effective_context_limit`: the program-safe context window for the selected model, accounting for response budget where known.
- `compaction_threshold`: a ratio or byte count at which compaction is triggered, such as 80 % of the effective limit.
- `reserved_recent_turns`: the minimum number of most recent turns to preserve uncompressed, or a token budget reserved for recent uncompressed content.

The trigger should evaluate before each model invocation where context assembly has produced a token estimate. If the estimate exceeds the threshold, compaction should run before the model call proceeds. This threshold-driven path is **automatic compaction**.

The user may also request compaction explicitly through a client command such as `/compact`. This user-requested path is **manual compaction**. Manual and automatic compaction share eligibility, summary, durable-recording, and context-snapshot behavior, but they differ in the user-visible started notice.

Compaction may be skipped when:

- There is insufficient eligible history to compact (only recent turns exist, or a prior compaction already summarized older history).
- Compaction would not produce meaningful token savings (the eligible range is too small).
- The current invocation is within a tool-call loop where the model is still completing tool work, and re-compacting the same prefix would add latency without new information. In this case, compaction may be deferred until the next user-initiated turn.
- A prior compaction for the same eligible range is still in progress or recently completed.

When compaction is skipped despite threshold pressure, the program should log or record the reason so future context assembly decisions are auditable.

## Eligibility

Context is divided into two conceptual regions for compaction:

1. **Preserved recent range**: The most recent N turns (or a token budget reserved for recent content) that are not compacted. These turns remain as full transcript items in the active context.
2. **Compaction-eligible range**: Older turns that are candidates for summarization.

Eligibility rules:

- Turns already summarized by a prior compaction are not eligible for re-compaction unless the summary itself has become stale and a new compaction of the same range would materially improve context quality.
- The current turn's user input and any in-progress tool work must not be compacted.
- Steer messages, queue items, and other active-turn records associated with uncompressed turns should be preserved with their parent turns or included in the summary where meaningful.
- A turn that was interrupted may be compacted, but the summary should note that work was interrupted.
- A turn that was superseded by message editing may be compacted; the summary should reflect the replacement branch content rather than the superseded turn.
- Fork-inherited history segments are eligible for compaction the same way as native transcript turns.

## Summary Content

The compaction summary is a durable record that replaces eligible transcript detail in future context snapshots. It is not a transcript item and does not appear in the user-visible conversation history as a user or assistant message.

The summary must preserve enough information for the model to continue work without the compacted raw detail. Required summary content:

- **Current objectives**: What the user asked for and what remains to be done, including any explicit task goals from the plan tool.
- **Key decisions**: Architectural choices, design tradeoffs, selected approaches, and rationale where recorded in the compacted turns.
- **Changed files**: Files created, modified, deleted, or renamed, grouped by turn where attribution is clear. Include enough path and change-kind information for the model to understand workspace state.
- **Blockers and unresolved work**: Any work that was blocked, deferred, interrupted, or requires follow-up.
- **Verification status**: Tests written, tests run, verification outcomes, and remaining test gaps where recorded in the compacted turns.
- **Error context**: Persistent errors, provider failures, or tool failures that may affect future work, with enough detail for the model to avoid repeating the same failure.

The summary may also include:

- Persona and mode changes that occurred during the compacted range, if those changes are not already captured by the current metadata-derived content.
- Notable tool outputs that constitute durable task state (e.g., a resolved approval, a completed plan item, a confirmed file path).
- A compacted representation of earlier change-signal messages, reduced to the fact that a change occurred rather than the full signal text.

The summary should omit:

- Transient assistant reasoning or exploration that did not lead to decisions or changes.
- Redundant tool output, repeated search results, or low-value content that carries no durable task state.
- Full verbatim assistant responses unless the response text itself records a decision or finding that cannot be derived from other summary fields.

## Compaction Flow

Conceptual compaction flow:

```text
Context assembly detects token estimate exceeds threshold
        ↓
Identify compaction-eligible turn range and preserved recent range
        ↓
Record durable context_compaction_started
        ↓
Emit transcript-area `Manual Compaction Started` or `Automatically Compaction Started` notice
        ↓
Extract summary content from eligible turns
        ↓
Build summary record (objectives, decisions, changed files, blockers, verification, errors)
        ↓
Record durable context_compaction_completed with summary reference
        ↓
Emit transcript-area `Compaction Done` notice
        ↓
Create updated context snapshot referencing summary plus preserved recent turns
        ↓
Proceed with model invocation using compacted context
```

Compaction must complete before the invocation that detected the threshold proceeds. If compaction fails, the program should record the failure and either proceed with uncompressed context (accepting provider-limit risk) or fail the turn with a structured context-overflow error.

## Durable Recording

Compaction produces these durable records through `L2-DES-CONV-001`:

- `context_compaction_started`: identifies the compaction event, the session, the trigger source (`manual` or `automatic`), the triggering invocation or command where applicable, the eligible turn range, the preserved recent range, and the compaction strategy.
- `context_compaction_completed`: references the compaction event, the produced summary record, the compacted turn range, the token estimate before and after compaction, and the new context snapshot reference.

The summary record itself is a durable context record, not a transcript item. It is stored as a content-addressable or identified record referenced by the context snapshot.

Conceptual summary record fields:

- `summary_id`
- `session_id`
- `compaction_event_id`
- `trigger_source`: manual or automatic.
- `compacted_turn_range`: first and last turn id in the compacted range.
- `preserved_recent_range`: first and last turn id in the preserved range, for traceability.
- `objectives`
- `decisions`
- `changed_files`: a structured list of paths, change kinds, and source turns.
- `blockers_and_unresolved`
- `verification_status`
- `error_context`
- `notable_state_changes`: persona, mode, or other metadata changes during the compacted range.
- `created_at`
- `content_hash`

## Active Context After Compaction

The active context snapshot after compaction should reference:

```text
[Immutable prefix — same as before compaction]
[Metadata-derived: persona instructions (current)]
[Metadata-derived: interaction-mode instructions (current)]
[Summary record — replacing compacted eligible turns]
[Preserved recent turns — uncompressed, as full transcript items]
[Consolidated change-signal message, if applicable]
[User input — current turn]
```

The immutable prefix is unchanged by compaction. The summary record is appended as a new context reference, not as an in-place mutation of earlier prefix content. The preserved recent turns remain as direct transcript references. This satisfies the token-efficiency requirement for stable prefixes: compaction does not rewrite earlier context bytes, it produces a new context snapshot that references different records.

When compaction runs again later, the prior summary becomes part of the compaction-eligible range. The new summary should incorporate the prior summary's preserved content so task continuity is not lost across multiple compactions.

## Replay and Recovery

After replay, the program must be able to:

- Reconstruct the compacted context snapshot from durable compaction records and summary records.
- Identify which transcript turns were compacted into which summary.
- Display the full raw transcript to the user for review, even when the active model context uses the summary.
- Detect whether a compaction event was recorded without a corresponding completion record (indicating a compaction that crashed) and either resume or restart compaction during the next context assembly.

Crash during compaction should leave durable records in a recoverable state: either the pre-compaction context snapshot remains valid and the incomplete compaction event can be discarded, or the compaction event is restartable from the durable `compaction_started` record.

## User Visibility

The full transcript remains available for user review regardless of compaction. Compaction affects only the model-visible context, not the user-visible conversation history.

The program should make compaction visible to the user through:

- A context status indicator showing the current token estimate relative to the effective limit.
- Transcript-area lifecycle notices with exact labels:
  - `Manual Compaction Started` when compaction was requested by the user.
  - `Automatically Compaction Started` when compaction was triggered by context pressure.
  - `Compaction Done` when compaction completes successfully.
- An indication of how many turns were compacted and how many remain uncompressed.
- The ability to inspect a summary record to understand what was preserved from compacted history.

The transcript-area notices are user-visible status cells. They are not user, assistant, or model-visible transcript messages, and they do not expose the summary content inline. Replay should be able to reconstruct the notices from durable compaction records.

The user should not be required to approve compaction for it to proceed during normal operation. Compaction is a context-management operation, not a user-prompted workflow.

## Invariants

- Compaction is triggered before model invocation, not during or after.
- Compaction creates new durable records and updated context snapshots; it never deletes or mutates existing transcript turns.
- The immutable prefix is not rewritten by compaction.
- Recent turns remain uncompressed; only older eligible history is compacted.
- A compaction summary must preserve objectives, decisions, changed files, blockers, verification status, and error context.
- The full transcript remains available for user review outside the model context.
- Compaction failures must not leave the session in an unrecoverable context state.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---:|---|---|---|
| refines | L1-REQ-CONTEXT-003 | 1 | specs/L1/L1-REQ-CONTEXT-003-compress.md | Defines compaction triggers, eligibility, summary content, durable recording, and context snapshot updates. |
| related-to | L1-REQ-CONTEXT-001 | 1 | specs/L1/L1-REQ-CONTEXT-001-management.md | Compaction is the primary mechanism for managing context growth across long sessions. |
| related-to | L1-REQ-LLM-001 | 1 | specs/L1/L1-REQ-LLM-001-token-efficiency.md | Compaction uses append-only summary records to avoid prefix mutation while reducing token usage. |
| related-to | L2-DES-CONV-001 | 1 | specs/L2/conv/L2-DES-CONV-001-session-jsonl-data-model.md | Defines durable compaction records, summary records, and context snapshot structure. |
| related-to | L2-DES-AGENT-001 | 1 | specs/L2/agent/L2-DES-AGENT-001-execution-engine.md | Compaction runs before model invocation within the execution engine's context assembly phase. |
| related-to | L2-DES-CONTEXT-001 | 1 | specs/L2/context/L2-DES-CONTEXT-001-context-assembly.md | Compaction updates active context snapshots while preserving the immutable prefix and metadata-derived content structure. |
| specified-by | TBD | TBD | specs/L3/context/TBD.md | L3 behavior has not been authored yet. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-22 | Assistant | Initial | Initial context compaction design covering triggers, eligibility, summary content, durable recording, active context update, and replay recovery. |
| 1 | 2026-05-25 | Human | Refinement | Added exact transcript-area lifecycle notices for manual and automatic compaction. |


---
artifact_id: L3-BEH-CORE-011
revision: 1
status: Draft
active_baseline: no
---

# L3-BEH-CORE-011 — Session Forking and Inherited History Retention

## Purpose

Define concrete behavior for `session/fork`, inherited-history segment creation, fork replay, parent deletion, cascade deletion, and client fork projections so forked sessions remain usable even when their parent session is later deleted or unavailable.

## Source Design

L1-REQ-CONV-004 (Session Forking), L2-DES-CONV-001 (Session JSONL Data Model), L2-DES-APP-003 (Client Server Protocol), L3-BEH-CORE-001 (Session JSONL Data Model and Replay)

## Problem Statement

A forked session has two different relationships to the parent:

- **Origin metadata**: where the fork came from, useful for display and navigation.
- **Inherited history**: the actual replayable content the fork needs for transcript rendering and model context.

These must not be collapsed into a single parent-session pointer. After parent deletion, `parent_session_id` and `fork_turn_id` may become tombstone provenance only. The fork must still replay from its own durable file plus an inherited-history segment that does not require opening the deleted parent file.

## Data Structures

### Fork Origin

`ForkOrigin` is stored in the child session's `session_forked` durable record and projected into session metadata.

| Field | Type | Required | Notes |
|---|---|---|---|
| `parent_session_id` | SessionId | yes | Provenance and navigation key. May become non-dereferenceable. |
| `fork_turn_id` | TurnId | yes | Turn boundary selected for fork. |
| `fork_created_at` | Timestamp | yes | Creation time. |
| `parent_display_label` | String | yes | Stable user-facing label captured at fork time. |
| `fork_turn_display_label` | String | yes | Stable turn label for fork indicator. |
| `fork_turn_digest` | String | yes | Short digest of selected turn content. |
| `origin_snapshot_hash` | String | yes | Hash of origin metadata plus inherited segment descriptor. |
| `parent_availability` | ParentAvailability | yes | `available`, `archived`, `deleted`, `unavailable`, or `unknown`. |

### Inherited History Segment

`InheritedHistorySegmentDescriptor` is stored in the child `session_forked` record.

| Field | Type | Required | Notes |
|---|---|---|---|
| `inherited_segment_id` | String | yes | Stable segment identifier. |
| `source_parent_session_id` | SessionId | yes | Provenance only after deletion. |
| `source_range` | SourceRange | yes | Parent record range used to build the segment. |
| `storage_strategy` | StorageStrategy | yes | See below. |
| `record_refs` | Vec<RecordRef> | yes | References or materialized record ids in replay order. |
| `segment_hash` | String | yes | SHA-256 of canonical segment content. |
| `availability_state` | SegmentAvailability | yes | `available`, `materialized`, `protected`, `missing`, or `corrupt`. |
| `created_at` | Timestamp | yes | Segment creation time. |

`StorageStrategy` values:

| Value | Meaning |
|---|---|
| `protected_shared_segment` | The selected inherited records are written to a content-addressed segment store independent of the parent session file. Multiple forks may reference the same segment. |
| `materialized_fork_segment` | The selected inherited records are materialized into the fork session's storage. |
| `protected_retained_source_records` | The fork references parent source records and the parent storage is protected from hard deletion until the fork is materialized or cascade-deleted. |

Default strategy should be `protected_shared_segment`. `protected_retained_source_records` is allowed only when the deletion policy can enforce retention before any parent hard purge.

### RecordRef

| Field | Type | Required | Notes |
|---|---|---|---|
| `source_session_id` | SessionId | yes | Source session for provenance. |
| `record_sequence` | u64 | yes | Durable sequence in source session. |
| `record_offset` | u64 | yes | Byte offset where available. |
| `record_kind` | String | yes | Record kind included in the segment. |
| `record_hash` | String | yes | Hash of canonical record content. |
| `materialized_ref` | String? | no | Segment-local id when copied into a shared or fork segment. |

## Behavior Specification

### B1. Fork Admission

- **Trigger**: Client sends `session/fork`.
- **Preconditions**: Parent session exists and the requester is allowed to read it.
- **Algorithm / Flow**:
  1. Validate `parent_session_id`.
  2. Validate `fork_turn_id` belongs to the parent session.
  3. Accept fork only at a stable boundary:
     - completed turn,
     - failed or interrupted terminal turn,
     - or an explicit context snapshot captured before subagent spawn.
  4. Reject active in-progress turn forks unless the execution engine first creates a forkable stable snapshot.
  5. Resolve `workspace_root`. If omitted, inherit the parent's workspace root from the fork point.
  6. Generate child `session_id` and `inherited_segment_id`.
  7. Build `ForkOrigin` display labels and `fork_turn_digest`.
- **Postconditions**: The server has a validated fork plan but has not yet made the child session visible.
- **Errors**: `ParentSessionNotFound`, `ForkTurnNotFound`, `ForkTurnNotStable`, `PermissionDenied`, `WorkspaceUnavailable`.

### B2. Inherited Segment Construction

- **Trigger**: Fork admission succeeds.
- **Preconditions**: Parent replay projection is available through `fork_turn_id`.
- **Algorithm / Flow**:
  1. Replay the parent session up to and including the selected fork turn.
  2. Select records needed to render and continue the fork:
     - session metadata needed for visible transcript interpretation,
     - visible user and assistant transcript items,
     - tool call/result summaries needed for understandable history,
     - attachment and artifact references needed by visible content parts,
     - context summaries or compaction outputs that replace visible earlier history,
     - plan/goal state only if it is visible or needed to interpret the fork point.
  3. Exclude records that must not become inherited visible history:
     - provider-only raw deltas superseded by coalesced content,
     - hidden goal continuation prompts,
     - plaintext credential material,
     - internal persistent-memory records,
     - unrelated background job state.
  4. Canonicalize the selected record projection.
  5. Write the inherited segment using the selected `storage_strategy`.
  6. Compute `segment_hash`.
  7. Fsync the segment before creating the child session.
- **Postconditions**: A replayable inherited segment exists independently of the child session record.
- **Error Handling**: If segment write or hash verification fails, abort the fork and do not create a visible child session.

### B3. Child Session Creation

- **Trigger**: Inherited segment construction succeeds.
- **Preconditions**: Segment has been fsynced and verified.
- **Algorithm / Flow**:
  1. Create the child session file.
  2. Append `session_created` for the child.
  3. Append `session_forked` for the child containing:
     - child `session_id`,
     - `ForkOrigin`,
     - `InheritedHistorySegmentDescriptor`,
     - `workspace_root`,
     - `fork_label`,
     - `created_by`: user, subagent, or system.
  4. Update the session index or projection to include the child and fork relation.
  5. Return `session/fork` response: `session_id`, `parent_session_id`, `fork_turn_id`, `inherited_segment_id`, and `session_snapshot`.
- **Postconditions**: The child session is durable and visible. Future turns in the child do not mutate the parent.

### B4. Fork Replay

- **Trigger**: `session/open`, `session/subscribe`, context assembly, export, or server restart loads a forked session.
- **Preconditions**: Child session file is readable.
- **Algorithm / Flow**:
  1. Replay child records normally.
  2. On `session_forked`, load the inherited-history segment through its descriptor.
  3. Verify `segment_hash`.
  4. Build the fork projection:
     - inherited transcript cells first,
     - visible fork indicator,
     - child-owned turns after the fork.
  5. If parent session is available, mark `parent_availability = available` and keep navigation target enabled.
  6. If parent session is deleted or unavailable but inherited segment verifies, mark `parent_availability = deleted` or `unavailable` and keep inherited history visible.
  7. If inherited segment is missing or corrupt:
     - If parent source records are still available and hashes match, repair by materializing a new segment and append/update metadata through a durable metadata update.
     - If neither segment nor source is available, mark the fork projection as `InheritedHistoryUnavailable` and return a structured recovery error. Do not silently render an empty inherited transcript.
- **Postconditions**: A valid fork renders inherited history without depending on parent availability.

### B5. Parent Deletion Preflight

- **Trigger**: Client sends `session/delete` for a session that may have fork descendants.
- **Preconditions**: Session index can find direct and transitive fork descendants.
- **Algorithm / Flow**:
  1. Discover affected forks where `ForkOrigin.parent_session_id` equals the target session or where protected retained source records depend on the target session.
  2. For every affected surviving fork, classify the inherited segment action:
     - `already_independent`: protected shared segment or materialized segment verifies.
     - `materialize_required`: fork uses protected retained source records.
     - `missing_or_corrupt`: segment cannot be verified.
     - `cascade_candidate`: fork will be deleted only if user explicitly requested cascade where supported.
  3. If destructive deletion would break any surviving fork and no materialization policy was requested, reject with `ForkRetentionRequired`.
  4. Return deletion preflight result with `affected_forks`, `inherited_segment_actions`, `retained_records`, and `confirm_token` if confirmation is required.
- **Postconditions**: The user can see whether deletion preserves, materializes, blocks, or cascades fork descendants.

### B6. Parent Deletion Commit

- **Trigger**: Client confirms `session/delete`.
- **Preconditions**: Preflight result is still valid or has been recomputed.
- **Algorithm / Flow**:
  1. Recompute affected forks to avoid stale confirmation.
  2. For every surviving fork:
     - If segment is already independent, no content migration is needed.
     - If `materialize_required`, materialize the inherited segment before deleting parent storage.
     - Append `session_metadata_updated` to the fork setting `fork_origin.parent_availability = deleted` and recording retained display labels.
  3. If cascade deletion is explicitly requested and supported, delete descendants according to cascade order from leaves to root.
  4. Append `session_deleted` to the parent with `delete_state`, `affected_forks`, `inherited_segment_actions`, and `retained_records`.
  5. Only after durable records and required materialization succeed may the parent session file or index entry be made inaccessible.
- **Postconditions**: Parent deletion cannot make a surviving fork unreplayable.
- **Failure Handling**: If materialization fails for any surviving fork, abort hard deletion and leave the parent available or tombstoned.

### B7. Client Projection

- **Trigger**: A forked session snapshot or event is sent to a client.
- **Preconditions**: Fork replay projection is available.
- **Algorithm / Flow**:
  1. Include `fork_origin` in `session_snapshot.metadata`.
  2. Include `inherited_segment_id`, `parent_availability`, and `navigation_available`.
  3. Render inherited transcript through the same transcript projection pipeline as live child turns.
  4. If parent navigation is unavailable, provide a clear state but keep inherited transcript visible.
  5. Never expose raw protected segment paths to clients unless an export or diagnostic explicitly permits it.
- **Postconditions**: Live and replayed forked sessions render consistently.

### B8. Subagent Forking

- **Trigger**: A subagent spawn requests `fork_turns = all` or a bounded turn count.
- **Preconditions**: Subagent feature is enabled and spawn admission succeeded.
- **Algorithm / Flow**:
  1. Resolve the parent fork point from the requested history mode.
  2. Create the inherited segment using B2.
  3. Create child session using B3 with `created_by = subagent`.
  4. Persist subagent spawn records after the child fork session is durable.
- **Postconditions**: Subagent inherited context is replayable without mutating parent history.

## Required Tests

- Fork from a completed turn creates a child with `session_created`, `session_forked`, and a verified inherited segment.
- Fork from an active turn is rejected unless a stable context snapshot exists.
- Child turns after fork do not modify the parent session file.
- Opening a fork with parent available renders inherited transcript plus child turns.
- Opening a fork after parent deletion still renders inherited transcript from the segment.
- Parent deletion preflight reports affected forks and required materialization actions.
- Hard parent deletion is blocked when a surviving fork still depends only on retained source records.
- Parent deletion materializes retained source records before making the parent inaccessible.
- Cascade deletion deletes descendants only after explicit cascade request.
- Missing inherited segment repairs from parent source when source records are available and hashes match.
- Missing inherited segment with deleted parent returns a structured recovery error rather than silently dropping history.
- Subagent fork creation persists both child fork data and subagent spawn data in the correct order.
- Client projection marks parent navigation unavailable while preserving inherited transcript display.

## Traceability

| L2 Source | Relationship |
|---|---|
| L2-DES-CONV-001 | specified-by |
| L2-DES-APP-003 | specified-by |
| L2-DES-AGENT-003 | related-to |

## Implementation Placement Guidance

- Core owns inherited segment construction, hashing, replay, and parent-deletion retention checks.
- Server owns the JSON-RPC request handling and confirmation flow for `session/fork` and `session/delete`.
- A session index or SQLite projection may accelerate descendant lookup, but the forked child session and inherited segment remain the replay authority.
- The inherited segment store should use content-addressed or reference-counted storage so multiple forks can share the same segment without duplicating the full parent history.

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-27 | Assistant | Initial | Initial session forking, inherited-history segment, parent deletion, replay, and client projection behavior. |

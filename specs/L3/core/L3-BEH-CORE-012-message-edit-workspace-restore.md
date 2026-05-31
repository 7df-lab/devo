---
artifact_id: L3-BEH-CORE-012
revision: 1
status: Draft
active_baseline: no
---

# L3-BEH-CORE-012 — Immediate Message Editing and Workspace Restoration

## Purpose

Define concrete behavior for `message/editPrevious`: eligibility, append-only edit records, queued-message edits, superseded turn projection, workspace restoration, replacement turn creation, replay, and client events.

## Source Design

L1-REQ-CONV-005 (Immediate Message Editing), L1-REQ-CHANGE-001 (Rollback and Recovery), L2-DES-CONV-001 (Session JSONL Data Model), L2-DES-APP-003 (Client Server Protocol), L2-DES-AGENT-001 (Execution Engine)

## Principles

- Editing is append-only. The original user item, original turn, assistant output, tool output, and side effects remain auditable.
- Only the immediately preceding eligible user-authored message in the current session branch can be edited.
- Workspace restoration is server/core-owned. Clients do not apply inverse patches.
- Default restoration is conservative: restore only when the current file still matches the expected post-turn state or another explicitly safe predicate.
- Diverged files are skipped and current content is preserved.
- Non-file side effects are not rolled back.

## Durable Records

### MessageEditRecordedRecord

| Field | Type | Required | Purpose |
|---|---|---|---|
| `schema_version` | u32 | yes | always 1 |
| `session_id` | SessionId | yes | owning session |
| `edit_id` | EditId | yes | stable edit identifier |
| `target_message_id` | ItemId | yes | original user-authored item |
| `replacement_message_id` | ItemId | yes | replacement user item |
| `target_turn_id` | TurnId? | no | original turn if the message already executed |
| `replacement_turn_id` | TurnId? | no | replacement turn when immediate execution is accepted |
| `queue_item_id` | QueueItemId? | no | queue item when editing queued content |
| `edited_content_parts` | Vec<ContentPart> | yes | replacement content |
| `edited_mentions` | Vec<Mention> | yes | replacement mentions |
| `workspace_restore_policy` | WorkspaceRestorePolicy | yes | default safe, skip, or configured policy |
| `edit_state` | EditState | yes | accepted, restore_pending, replacement_started, queued_updated, rejected |
| `requested_by_client_id` | String? | no | client provenance |
| `created_at` | Timestamp | yes | acceptance time |

### TurnSupersededRecord

| Field | Type | Required | Purpose |
|---|---|---|---|
| `schema_version` | u32 | yes | always 1 |
| `session_id` | SessionId | yes | owning session |
| `superseded_turn_id` | TurnId | yes | original turn |
| `replacement_turn_id` | TurnId | yes | new branch turn |
| `edit_id` | EditId | yes | edit causing supersession |
| `restore_id` | RestoreId? | no | restore attempt used before replacement |
| `reason` | String | yes | normally `message_edit_previous` |
| `created_at` | Timestamp | yes | |

### TurnWorkspaceCheckpointRecordedRecord

| Field | Type | Required | Purpose |
|---|---|---|---|
| `checkpoint_id` | CheckpointId | yes | turn checkpoint id |
| `session_id` | SessionId | yes | owning session |
| `turn_id` | TurnId | yes | turn being checkpointed |
| `workspace_root` | Path | yes | workspace path |
| `checkpoint_strategy` | CheckpointStrategy | yes | structured inverse, hidden git checkpoint, filesystem snapshot, unsupported |
| `baseline_refs` | Vec<FileStateRef> | yes | pre-turn states where captured |
| `created_at` | Timestamp | yes | |

### TurnWorkspaceChangeRecordedRecord

| Field | Type | Required | Purpose |
|---|---|---|---|
| `change_set_id` | ChangeSetId | yes | per-turn change set |
| `file_change_id` | FileChangeId | yes | per-file change id |
| `session_id` | SessionId | yes | owning session |
| `turn_id` | TurnId | yes | changed turn |
| `tool_call_id` | String? | no | source tool |
| `tool_name` | String? | no | source tool name |
| `path_before` | Path? | no | previous path for rename/delete |
| `path_after` | Path? | no | final path for create/rename |
| `change_kind` | ChangeKind | yes | create, modify, delete, rename, mode_change |
| `pre_state_ref` | FileStateRef? | no | pre-turn content/state |
| `pre_state_hash` | String? | no | pre-turn hash or absence marker |
| `post_state_ref` | FileStateRef? | no | post-turn content/state |
| `post_state_hash` | String? | no | post-turn hash or absence marker |
| `inverse_ref` | InverseOperationRef? | no | structured inverse if available |
| `attribution_confidence` | AttributionConfidence | yes | structured, checkpointed, inferred, unknown |
| `recorded_at` | Timestamp | yes | |

### TurnWorkspaceRestoreStartedRecord

| Field | Type | Required | Purpose |
|---|---|---|---|
| `restore_id` | RestoreId | yes | restore attempt id |
| `session_id` | SessionId | yes | owning session |
| `edit_id` | EditId | yes | edit requiring restore |
| `superseded_turn_id` | TurnId | yes | turn being restored from |
| `checkpoint_id` | CheckpointId? | no | checkpoint used |
| `candidate_files` | Vec<Path> | yes | files considered |
| `restore_policy` | WorkspaceRestorePolicy | yes | applied policy |
| `started_at` | Timestamp | yes | |

### TurnWorkspaceRestoreCompletedRecord

| Field | Type | Required | Purpose |
|---|---|---|---|
| `restore_id` | RestoreId | yes | restore attempt id |
| `session_id` | SessionId | yes | owning session |
| `edit_id` | EditId | yes | edit requiring restore |
| `superseded_turn_id` | TurnId | yes | turn restored from |
| `results` | Vec<FileRestoreResult> | yes | per-file results |
| `summary` | RestoreSummary | yes | counts by status |
| `completed_at` | Timestamp | yes | |

`FileRestoreResult.restore_status` values:

| Status | Meaning |
|---|---|
| `restored` | File was restored to pre-turn state. |
| `skipped_current_state_kept` | File diverged or policy required preserving current content. |
| `unsupported` | No reliable restore data exists. |
| `failed` | Restore was attempted and failed. |
| `not_needed` | Current state already equals pre-turn state. |

## Behavior Specification

### B1. Edit Eligibility

- **Trigger**: Client sends `message/editPrevious`.
- **Preconditions**: Session exists and client is authorized.
- **Algorithm / Flow**:
  1. Load current branch projection.
  2. Identify the immediately preceding eligible user-authored message.
  3. Compare it to `expected_target_message_id`.
  4. Reject direct edits of older historical messages with `OlderMessageRequiresFork` and include the relevant fork suggestion.
  5. Reject active running turn edits by default with `ActiveTurnEditRejected`; the response may suggest `turn/interrupt` or steer.
  6. Validate replacement content parts and mentions using normal `turn/submit` validation.
- **Postconditions**: The request is either rejected or classified as queued edit or superseded-turn edit.

### B2. Queued Message Edit

- **Trigger**: Target message belongs to a queued item that has not started execution.
- **Algorithm / Flow**:
  1. Append `message_edit_recorded` with `queue_item_id`, original target id, replacement id, edited content, edited mentions, and `edit_state = queued_updated`.
  2. Update the queue projection so future execution uses the replacement content.
  3. Broadcast `message_edit_recorded`.
- **Postconditions**: The queue item effective content changes, but original queued content remains auditable.

### B3. Completed or Terminal Turn Edit

- **Trigger**: Target message belongs to the latest completed, failed, or interrupted turn in the current branch.
- **Algorithm / Flow**:
  1. Allocate `edit_id`, `replacement_message_id`, and `replacement_turn_id`.
  2. Append `message_edit_recorded` with `edit_state = restore_pending` when restoration will run, or `accepted` when `workspace_restore_policy = skip`.
  3. If restoration is not skipped, run B4-B7 before replacement turn admission.
  4. Append `turn_superseded` linking `superseded_turn_id`, `replacement_turn_id`, `edit_id`, and optional `restore_id`.
  5. Append replacement user `item_started`/`item_completed` records using the edited content and mentions.
  6. Admit replacement turn through the normal execution engine.
  7. Broadcast `message_edit_recorded`, restore events where applicable, `turn_superseded`, and normal replacement turn events in sequence order.
- **Postconditions**: The current branch uses the replacement turn. The superseded turn remains recoverable in audit projection.

### B4. Restore Candidate Selection

- **Trigger**: A superseded turn edit requires restoration.
- **Preconditions**: The superseded turn has zero or more workspace change records.
- **Algorithm / Flow**:
  1. Load all `turn_workspace_change_recorded` records for the superseded turn.
  2. If no change set exists, append `turn_workspace_restore_started` and `turn_workspace_restore_completed` with empty results and summary `unsupported_count = 0`.
  3. Merge repeated changes to the same final path into a restore plan that uses:
     - earliest pre-turn state,
     - latest expected post-turn state,
     - strongest available inverse or checkpoint reference.
  4. Include shell-command changes only when a checkpoint or attribution record has reliable file-level state.
  5. Exclude non-file side effects and report them only through the superseded turn audit trail.
- **Postconditions**: Candidate files are stable and ordered deterministically.

### B5. Safe Restore Predicate

- **Trigger**: Restore plan evaluates each candidate file.
- **Algorithm / Flow**:
  1. Read current file state. Missing files are represented by a stable absence marker.
  2. If current state equals expected post-turn state, restoration is safe.
  3. If current state equals pre-turn state, record `not_needed`.
  4. If current state differs from expected post-turn state and pre-turn state, record `skipped_current_state_kept`.
  5. A configured checkpoint strategy may define another safe predicate, but it must still prove that user changes after the superseded turn will not be overwritten.
- **Postconditions**: Diverged files are not overwritten by default.

### B6. Per-File Restore Actions

- **Trigger**: A candidate file passes the safe restore predicate.
- **Algorithm / Flow**:
  1. `create`: if the turn created the file and current state equals post-turn state, delete the file or restore absence.
  2. `modify`: write `pre_state_ref` content or apply inverse operation.
  3. `delete`: recreate from `pre_state_ref` only when the file is still absent.
  4. `rename`: move `path_after` back to `path_before` only when `path_after` equals expected post-state and `path_before` is absent or equals expected pre-state.
  5. `mode_change`: restore mode metadata when captured and current content predicate is safe.
  6. After action, verify restored hash equals `pre_state_hash` or absence marker.
- **Postconditions**: Each file has one explicit restore result.
- **Error Handling**: Failed write, delete, rename, or hash verification records `failed` and preserves the best-known current state.

### B7. Hidden Git Checkpoint Use

- **Trigger**: Restore plan has `checkpoint_strategy = hidden_git_checkpoint`.
- **Rules**:
  - Treat the checkpoint as internal content-addressed restore data, not as a user-visible commit.
  - Do not publish, stage, commit, reset visible branch history, or rewrite user-visible git state as part of default restoration.
  - Do not run a whole-workspace reset as the default restore action.
  - Use the checkpoint to recover pre-turn file content per file, then apply B5 and B6.
  - If checkpoint lookup fails, fall back to structured inverse records. If no inverse exists, mark the file `unsupported`.

### B8. Replay Projection

- **Trigger**: Session replay encounters edit and restore records.
- **Algorithm / Flow**:
  1. Preserve original message and original turn in audit projection.
  2. In current-branch projection, mark superseded turn as collapsed or superseded according to client projection settings.
  3. Use replacement message and replacement turn as the current branch continuation.
  4. Attach restore summary to the edit projection so clients can show restored, skipped, unsupported, and failed files.
  5. Do not infer restore completion from `turn_diff_updated`; only `turn_workspace_restore_completed` is authoritative.
- **Postconditions**: Live and replayed sessions present the same branch state and audit trail.

## Required Tests

- Editing older historical message returns `OlderMessageRequiresFork`.
- Stale `expected_target_message_id` returns a structured stale error.
- Active running turn edit is rejected unless a later approved interrupt-edit mode is implemented.
- Queued message edit updates effective queue content while preserving original revision.
- Completed turn edit appends `message_edit_recorded`, restore records, `turn_superseded`, replacement user item, and replacement turn in order.
- Created file is deleted when current state still equals the post-turn created content.
- Modified file is restored when current hash equals `post_state_hash`.
- Deleted file is recreated when still absent.
- Renamed file is moved back only when both source and destination predicates are safe.
- Diverged file records `skipped_current_state_kept` and is not overwritten.
- Shell-created file change without checkpoint records `unsupported`.
- Hidden git checkpoint never performs a whole-workspace reset in default restore mode.
- Replacement turn starts after restore completion when restore is requested.
- Replay projects replacement branch while retaining superseded turn in audit projection.
- Subscribed clients receive edit, restore, superseded-turn, and replacement-turn events in sequence order.

## Traceability

| L2 Source | Relationship |
|---|---|
| L2-DES-CONV-001 | specified-by |
| L2-DES-APP-003 | specified-by |
| L2-DES-AGENT-001 | related-to |

## Implementation Placement Guidance

- Core owns edit eligibility, restore planning, restore execution, and branch projection.
- Server owns JSON-RPC request handling and event broadcast sequencing.
- Structured mutating tools should report file changes into the core change-set accumulator before terminal tool result emission.
- Client-visible diffs are display projections only and must not be used as the restore authority.

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-27 | Assistant | Initial | Initial immediate-message-edit and workspace-restore behavior. |

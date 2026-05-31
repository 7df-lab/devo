---
artifact_id: L3-BEH-CORE-001
revision: 6
status: Draft
active_baseline: no
---

# L3-BEH-CORE-001 — Session JSONL Data Model and Replay

## Purpose

Define the complete durable data model: every JSONL record type with field-level schema, append timing rules, replay algorithm, crash recovery, and the `SessionStore` trait that `server` calls.

## Source Design

L2-DES-CONV-001, L3-DES-ARCH-001

## 1. SessionStore Trait (core → server contract)

```rust
/// Defined in core, implemented by core.
/// Server calls these methods; core owns all persistence decisions.
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Append one durable record. Blocks until fsync'd (or batched).
    async fn append(&self, session_id: SessionId, record: DurableRecord) -> Result<u64, StoreError>;

    /// Replay all records from a session, optionally from a byte offset.
    async fn replay(&self, session_id: SessionId, from_offset: u64)
        -> Result<ReplayStream, StoreError>;

    /// Flush any buffered appends to disk.
    async fn flush(&self, session_id: SessionId) -> Result<(), StoreError>;

    /// Return the current file size (used for offset tracking).
    async fn file_size(&self, session_id: SessionId) -> Result<u64, StoreError>;
}

pub struct StoreError {
    pub code: StoreErrorCode,
    pub message: String,
}

pub enum StoreErrorCode {
    SessionNotFound,
    FileCorrupted,
    DiskFull,
    PermissionDenied,
    IoError,
}
```

## 2. DurableRecord Enum — All Record Types

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "record_kind", rename_all = "snake_case")]
pub enum DurableRecord {
    // Session lifecycle
    SessionCreated(SessionCreatedRecord),
    SessionForked(SessionForkedRecord),
    SessionMetadataUpdated(SessionMetadataUpdatedRecord),
    SessionDeleted(SessionDeletedRecord),

    // Transcript
    TurnStarted(TurnStartedRecord),
    TurnCompleted(TurnCompletedRecord),
    TurnFailed(TurnFailedRecord),
    TurnInterrupted(TurnInterruptedRecord),
    TurnSuperseded(TurnSupersededRecord),
    ItemStarted(ItemStartedRecord),
    ItemContentAppended(ItemContentAppendedRecord),
    ItemCompleted(ItemCompletedRecord),
    ItemFailed(ItemFailedRecord),

    // Active turn messages
    SteerRecorded(SteerRecordedRecord),
    QueueItemRecorded(QueueItemRecordedRecord),
    QueueItemResolved(QueueItemResolvedRecord),

    // Interrupt/resume and background work
    TurnInterruptRequested(TurnInterruptRequestedRecord),
    TurnResumeStarted(TurnResumeStartedRecord),
    BackgroundProcessUpdated(BackgroundProcessUpdatedRecord),

    // Message editing
    MessageEditRecorded(MessageEditRecordedRecord),

    // Workspace
    TurnWorkspaceCheckpointRecorded(TurnWorkspaceCheckpointRecordedRecord),
    TurnWorkspaceChangeRecorded(TurnWorkspaceChangeRecordedRecord),
    TurnWorkspaceRestoreStarted(TurnWorkspaceRestoreStartedRecord),
    TurnWorkspaceRestoreCompleted(TurnWorkspaceRestoreCompletedRecord),

    // Plan
    PlanCreated(PlanCreatedRecord),
    PlanUpdated(PlanUpdatedRecord),

    // Goal
    GoalCreated(GoalCreatedRecord),
    GoalReplaced(GoalReplacedRecord),
    GoalStatusChanged(GoalStatusChangedRecord),
    GoalBudgetAccounted(GoalBudgetAccountedRecord),
    GoalProgressRecorded(GoalProgressRecordedRecord),
    GoalContextSnapshotRecorded(GoalContextSnapshotRecordedRecord),
    GoalCleared(GoalClearedRecord),

    // Context
    ContextSnapshotRecorded(ContextSnapshotRecordedRecord),
    ContextCompactionStarted(ContextCompactionStartedRecord),
    ContextCompactionCompleted(ContextCompactionCompletedRecord),

    // Usage
    UsageRecorded(UsageRecordedRecord),

    // Memory (internal)
    MemoryLinkRecorded(MemoryLinkRecordedRecord),

    // Subagents
    SubagentSpawned(SubagentSpawnedRecord),
    SubagentClosed(SubagentClosedRecord),
    SubagentMailRecorded(SubagentMailRecordedRecord),
    SubagentStatusChanged(SubagentStatusChangedRecord),
    SubagentNotificationRecorded(SubagentNotificationRecordedRecord),
}
```

## 3. Field-Level Schema — Key Record Types

### SessionCreatedRecord

| Field | Type | Required | Constraints |
|---|---|---|---|
| `schema_version` | u32 | yes | always 1 |
| `session_id` | SessionId (UUID) | yes | |
| `workspace_root` | String (canonical path) | yes | must be absolute |
| `created_at` | DateTime<Utc> | yes | |

**Write timing**: Immediately on `session/create` acceptance, before returning to client.

### SessionForkedRecord

Session forking and inherited-history retention behavior is specified by `L3-BEH-CORE-011`.

| Field | Type | Required | Constraints |
|---|---|---|---|
| `schema_version` | u32 | yes | always 1 |
| `session_id` | SessionId | yes | child fork session |
| `fork_origin` | ForkOrigin | yes | parent provenance and display metadata |
| `inherited_segment` | InheritedHistorySegmentDescriptor | yes | replayable inherited history descriptor |
| `workspace_root` | String | yes | absolute path |
| `fork_label` | Option<String> | no | user-facing label |
| `created_by` | ForkCreator | yes | `User`, `Subagent`, or `System` |
| `created_at` | DateTime<Utc> | yes | |

**Write timing**: After the inherited segment is written, fsynced, and hash-verified; before the fork is returned to the client or used by a subagent.

**Replay rule**: Replay loads and verifies the inherited segment before projecting inherited transcript content. `parent_session_id` and `fork_turn_id` are provenance keys, not the only content pointers.

### TurnStartedRecord

| Field | Type | Required | Constraints |
|---|---|---|---|
| `schema_version` | u32 | yes | |
| `session_id` | SessionId | yes | |
| `turn_id` | TurnId (UUID) | yes | |
| `sequence` | u32 | yes | monotonic within session, starts at 0 |
| `status` | TurnStatus | yes | always `Running` |
| `kind` | TurnKind | yes | `User \| SubAgent \| GoalContinuation \| Resume \| Steer` |
| `resume_of_turn_id` | Option<TurnId> | no | present iff kind == Resume |
| `submitted_by_client_id` | Option<String> | no | |
| `model` | Option<String> | no | resolved model slug, set during invocation |
| `thinking` | Option<String> | no | thinking/reasoning mode |
| `reasoning_effort` | Option<ReasoningEffort> | no | |
| `started_at` | DateTime<Utc> | yes | |

**Write timing**: Immediately after turn admission, before context assembly begins.

### ItemStartedRecord

| Field | Type | Required | Constraints |
|---|---|---|---|
| `schema_version` | u32 | yes | |
| `session_id` | SessionId | yes | |
| `turn_id` | TurnId | yes | |
| `item_id` | ItemId (UUID) | yes | |
| `kind` | ItemKind | yes | `UserInput \| AssistantText \| AssistantReasoning \| ToolCall \| ToolResult \| ApprovalRequest \| QuestionRequest \| SteerMessage \| QueueMessage \| Error \| ContextSummary` |
| `role` | Role | yes | `User \| Assistant \| Tool \| System` |
| `content_parts` | Vec<ContentPart> | yes | can be empty for streaming items |
| `mentions` | Vec<Mention> | no | present only for UserInput items |
| `visibility` | ItemVisibility | yes | `Visible \| Hidden \| Internal` |
| `created_at` | DateTime<Utc> | yes | |

**Write timing**: Before any content is sent to the model or client for this item.

### ItemContentAppendedRecord

| Field | Type | Required | Constraints |
|---|---|---|---|
| `schema_version` | u32 | yes | |
| `item_id` | ItemId | yes | must reference an open ItemStarted |
| `content_part_index` | u32 | yes | 0-based index into content_parts |
| `offset` | u64 | yes | byte offset within the logical content |
| `content_kind` | ContentAppendKind | yes | `Text \| Reasoning \| ToolCallJson \| ToolResultText` |
| `content` | String | yes | coalesced content bytes |
| `byte_count` | u32 | yes | length of content in bytes |

**Write timing**: Coalesced. Flushed on: 4096 bytes accumulated per part, 500ms elapsed, or semantic boundary (end of reasoning block, tool call completed, final assistant text). Max interval between appends: 1 second (safety flush).

**Replay rule**: All append records for an item are concatenated in `offset` order to reconstruct the item's full content per `content_part_index`.

### ItemCompletedRecord / ItemFailedRecord

| Field | Type | Required | Constraints |
|---|---|---|---|
| `item_id` | ItemId | yes | |
| `turn_id` | TurnId | yes | |
| `final_status` | ItemStatus | yes | `Completed \| Failed \| Interrupted \| Denied \| Blocked \| Canceled` |
| `content_hash` | Option<String> (SHA-256 hex) | no | |
| `error` | Option<ItemError> | no | present iff final_status is Failed |
| `completed_at` | DateTime<Utc> | yes | |

### TurnCompletedRecord / TurnFailedRecord / TurnInterruptedRecord

| Field | Type | Required | Constraints |
|---|---|---|---|
| `turn_id` | TurnId | yes | |
| `session_id` | SessionId | yes | |
| `status` | TurnStatus | yes | `Completed \| Failed \| Interrupted` |
| `usage` | Option<TurnUsage> | no | |
| `workspace_change_set_id` | Option<String> | no | |
| `completed_at` | DateTime<Utc> | yes | |
| `error` | Option<TurnError> | no | present iff Failed |

**Write timing**: Last record written for a turn. Must be fsync'd before responding to client.

### UsageRecordedRecord

| Field | Type | Required | Constraints |
|---|---|---|---|
| `schema_version` | u32 | yes | always 1 |
| `session_id` | SessionId | yes | |
| `turn_id` | TurnId | yes | |
| `invocation_id` | InvocationId | yes | one model call |
| `model_binding_id` | ModelBindingId | yes | binding used for the invocation |
| `canonical_model_slug` | String | yes | supported-model slug |
| `provider_id` | ProviderId | yes | effective provider id |
| `invocation_method` | InvocationMethod | yes | normalized provider SDK/method |
| `reasoning_effort` | Option<ReasoningEffort> | no | present when used |
| `metrics` | Vec<UsageMetric> | yes | normalized metrics with source labels |
| `context_pressure` | ContextPressure | yes | context size, effective limit, and pressure state |
| `recorded_at` | DateTime<Utc> | yes | |

`UsageMetric` and `ContextPressure` are specified by `L3-BEH-PROVIDER-003`. Values must distinguish provider-reported, locally estimated, unavailable, and redacted data. Missing provider usage is represented as unavailable, not zero.

**Write timing**: When provider usage is received, when invocation completes, or when a turn ends without provider usage. Usage should be written before the terminal turn record where possible.

**Replay rule**: Replay stores invocation-level usage, then derives turn and session totals from compatible numeric metrics. Replay must preserve source labels and must not double-count reasoning tokens whose inclusion relationship is unknown.

### Subagent Durable Records

Subagent record schemas are specified by `L3-BEH-SERVER-003`.

| Record | Required Replay Behavior |
|---|---|
| `SubagentSpawned` | Create an open parent-child edge and register child metadata under the root session projection. |
| `SubagentClosed` | Mark the edge closed without deleting the child transcript. |
| `SubagentMailRecorded` | Rebuild recipient mailbox queues in sequence order. |
| `SubagentStatusChanged` | Update visible subagent status in the root session projection. |
| `SubagentNotificationRecorded` | Preserve completion notification delivery and prevent duplicate watcher notification on replay. |

These records are durable session records, not only SQLite graph rows. SQLite or another graph store may index them, but replay must be able to rebuild the graph projection from the JSONL record stream and child session metadata.

### Interrupt and Background Process Records

Interrupt and background process behavior is specified by `L3-BEH-SERVER-002`.

| Record | Required Replay Behavior |
|---|---|
| `TurnInterruptRequested` | Preserve audit/provenance for accepted interruption requests. Does not by itself mark a turn terminal. |
| `TurnResumeStarted` | Preserve audit/provenance for accepted resume requests before the linked resume turn starts. |
| `BackgroundProcessUpdated` | Rebuild tracked background-process projection, including detached-visible processes and stop outcomes. |

Resume uses both `TurnResumeStarted` and `TurnStartedRecord`. `TurnResumeStarted` records the accepted resume request and allocated resume turn id; `TurnStartedRecord` records the actual admitted turn with `kind = Resume` and `resume_of_turn_id` referencing the interrupted turn.

### Message Edit and Workspace Restore Records

Immediate message editing and workspace restoration behavior is specified by `L3-BEH-CORE-012`.

| Record | Required Replay Behavior |
|---|---|
| `MessageEditRecorded` | Preserve original and replacement message relationship and update current-branch projection. |
| `TurnSuperseded` | Mark original turn as superseded in current branch while keeping audit projection recoverable. |
| `TurnWorkspaceCheckpointRecorded` | Preserve pre-turn restore data or checkpoint references. |
| `TurnWorkspaceChangeRecorded` | Preserve per-file attribution, pre/post hashes, inverse refs, and display diff references. |
| `TurnWorkspaceRestoreStarted` | Preserve restore attempt start, candidate files, and policy. |
| `TurnWorkspaceRestoreCompleted` | Preserve authoritative per-file restore outcomes. |

Replay must not infer restoration from client-visible `turn_diff_updated` events. Only durable workspace restore records are authoritative for restored/skipped/unsupported/failed file outcomes.

## 4. State Machine — Turn Lifecycle

```
                    ┌──────────┐
          ┌────────>│ Admitted │
          │         └────┬─────┘
          │              │ (context assembly starts)
          │         ┌────▼─────┐
          │         │  Running  │──────────────┐
          │         └────┬─────┘              │
          │              │                    │
          │    ┌─────────┼─────────┐          │
          │    │         │         │          │
          │    ▼         ▼         ▼          │
          │ ┌──────┐ ┌──────┐ ┌────────┐     │
          │ │Wait  │ │Tool  │ │Generat │     │
          │ │Approval│ │Dispatch│ │ing    │     │
          │ └──┬───┘ └──┬───┘ └───┬────┘     │
          │    │         │         │          │
          │    └─────────┼─────────┘          │
          │              │ (all subtasks done) │
          │         ┌────▼─────┐              │
          │         │Finalizing│              │
          │         └────┬─────┘              │
          │              │                    │
          │    ┌─────────┼─────────┐          │
          │    ▼         ▼         ▼          │
          │ ┌────────┐┌────────┐┌──────────┐  │
          │ │Completed││ Failed ││Interrupted│  │
          │ └────────┘└────────┘└────┬─────┘  │
          │                          │        │
          │         (resume)         │        │
          └──────────────────────────┘        │
                    (interrupt)               │
                    └─────────────────────────┘
```

### Legal Transitions

| From | To | Condition |
|---|---|---|
| (none) | `Admitted` | `turn/submit` accepted |
| `Admitted` | `Running` | Context assembly begins |
| `Admitted` | `Failed` | Context assembly or model resolution fails |
| `Running` | `Failed` | Any unrecoverable phase error |
| `Running` | `Interrupted` | Interrupt requested, execution yields |
| `Running` | `Completed` | Model produces terminal response, all tools done, finalization succeeds |
| `Interrupted` | `Admitted` (new turn) | `turn/resume` creates linked continuation |
| `Completed` | `Completed` | Idempotent — no-op |

### Illegal Transitions (MUST be rejected)

| From | To | Why Illegal |
|---|---|---|
| `Admitted` | `Running` (duplicate) | At most one turn active per session |
| `Completed` | `Running` | Terminal state is final |
| `Failed` | `Completed` | Cannot retroactively succeed |
| `Interrupted` | `Completed` | Must go through resume (new turn) |
| `Running` | `Running` (without interrupt) | Turn is already executing |

## 5. Replay Algorithm

```
fn replay(session_id, from_offset) -> ReplayResult:
    projections:
        metadata: SessionMetadata        // latest
        turns: Vec<TurnProjection>       // all turns in order
        active_plan: Option<PlanProjection>
        active_goal: Option<GoalProjection>
        context_snapshot: Option<ContextSnapshot>
        usage_totals: UsageTotals
        loaded_deferred_tools: HashSet<String>

    pending_items: HashMap<ItemId, StreamingItem>

    open file at from_offset
    for each line:
        record = parse_jsonl(line)
        match record.kind:
            SessionCreated => metadata.created_at = ts
            SessionForked => load inherited segment, verify hash, update fork_origin projection
            SessionMetadataUpdated => apply patch to metadata
            TurnStarted => push new TurnProjection, status=Running
            TurnInterruptRequested => record interrupt audit/provenance
            TurnResumeStarted => record resume audit/provenance
            ItemStarted => create StreamingItem in pending_items
            ItemContentAppended => append content to pending_items[item_id].parts[content_part_index] at offset
            ItemCompleted => finalize item, remove from pending_items
            ItemFailed => finalize item with error
            TurnCompleted|TurnFailed|TurnInterrupted => set turn status, finalize workspace_change_set
            MessageEditRecorded => record edit relationship and replacement message
            TurnSuperseded => mark turn superseded in current branch projection
            TurnWorkspaceCheckpointRecorded => record workspace checkpoint
            TurnWorkspaceChangeRecorded => accumulate workspace change set
            TurnWorkspaceRestoreStarted => record restore attempt start
            TurnWorkspaceRestoreCompleted => record restore outcome summary
            PlanCreated|PlanUpdated => update active_plan
            GoalCreated|GoalReplaced|GoalStatusChanged|GoalCleared => update active_goal
            GoalBudgetAccounted|GoalProgressRecorded => update active_goal counters
            ContextSnapshotRecorded => update context_snapshot
            ContextCompactionStarted => record pending compaction
            ContextCompactionCompleted => apply compaction summary to context_snapshot
            UsageRecorded => accumulate into usage_totals
            BackgroundProcessUpdated => update background_process projection
            SubagentSpawned => update agent_tree with open edge
            SubagentClosed => mark agent_tree edge closed
            SubagentMailRecorded => enqueue mailbox message by recipient and sequence
            SubagentStatusChanged => update agent_tree status
            SubagentNotificationRecorded => record delivered subagent notification
            // ... etc

    // Post-replay: resolve unterminated state
    for (item_id, item) in pending_items:
        if item has no ItemCompleted/ItemFailed:
            append ItemFailed(item_id, "Session terminated unexpectedly")
    for turn in turns where status == Running:
        append TurnInterrupted(turn.turn_id)
    for compaction_started without compaction_completed:
        discard compaction, keep pre-compaction snapshot

    return ReplayProjection { metadata, turns, active_plan, active_goal, context_snapshot, usage_totals }
```

## 6. Async Behavior

| Operation | Timeout | Retry | Cancel |
|---|---|---|---|
| `SessionStore::append` | 1s flush timeout | Retry once on IoError | Abort on CancellationToken |
| `SessionStore::replay` | No timeout (blocking load) | None | N/A (startup operation) |
| `SessionStore::flush` | 1s | Retry once | Abort on CancellationToken |
| Content coalescence flush | 500ms max delay | N/A | Flush remaining on cancel |

## 7. Edge Cases

- **Zero-length session**: `SessionCreated` exists but no turns. Replay produces empty projection.
- **Truncated file**: Last line is partial JSON → discard. Unterminated turn → mark interrupted.
- **Concurrent writers**: JSONL is single-writer. Server serializes all appends through one async task per session.
- **Disk full during append**: `DiskFull` error. Turn that was writing fails. Previously written records are safe.
- **Schema version bump**: Old records with unknown version → skip with warning. Known version with migration → run migration.

## Traceability

| L2 Source | Relationship |
|---|---|
| L2-DES-CONV-001 | specified-by |
| L3-DES-ARCH-001 | specified-by |

## Implementation Placement Guidance

- JSONL file at `<data_dir>/sessions/<session_id>.jsonl`. One file per session.
- `schema_version` field on every record for forward compatibility.
- Write-ahead: buffer appends, flush on terminal records. If crash before flush, unflushed data is lost but session state is recoverable as interrupted.
- Replay uses `serde_json::StreamDeserializer` for memory-efficient streaming parse.
- Existing protocol DTOs may be reused only when they are pure data and match this L3 contract. `DurableRecord` is a core-owned persistence enum and must not be weakened to fit client-only projections.

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-27 | Assistant | Initial | Initial durable session JSONL data model and replay behavior. |
| 2 | 2026-05-27 | Assistant | Correction | Added invocation usage records and provider usage replay behavior. |
| 3 | 2026-05-27 | Assistant | Correction | Added durable subagent record family and replay hooks required by subagent coordination L3. |
| 4 | 2026-05-27 | Assistant | Correction | Added interrupt/resume/background process durable record family and replay hooks. |
| 5 | 2026-05-27 | Assistant | Correction | Added SessionForked field schema and replay rule tied to inherited-history retention L3. |
| 6 | 2026-05-27 | Assistant | Correction | Added message edit and workspace restore durable record replay hooks tied to L3-BEH-CORE-012. |

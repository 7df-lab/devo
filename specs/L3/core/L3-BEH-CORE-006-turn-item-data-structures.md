---
artifact_id: L3-BEH-CORE-006
revision: 3
status: Draft
active_baseline: no
---

# L3-BEH-CORE-006 — Turn, Item, and Transcript Data Structures

## Purpose

Define the concrete Rust types and behavioral contracts for `TurnMetadata`, items, content parts, mentions, plan state, goal state, workspace change tracking, and edit records as specified by L2-DES-CONV-001.

## Source Design

L2-DES-CONV-001 (Session JSONL Data Model), L2-DES-AGENT-001 (Execution Engine)

## Behavior Specification

### B1. TurnMetadata Lifecycle

- **Trigger**: A turn is admitted, executes, and reaches a terminal state.
- **Preconditions**: `SessionId` and `TurnId` are assigned. The turn has a defined `TurnKind`.
- **Algorithm / Flow**:
  1. On admission: create `TurnMetadata` with `status: Running`, `kind` set to `User`, `SubAgent`, `GoalContinuation`, or `Resume`, `started_at` set to now, all optional fields `None`.
  2. During execution: populate `model`, `thinking`, `reasoning_effort` from the resolved model profile. Update `request_model` and `request_thinking` from the provider request.
  3. On terminal: set `completed_at` to now, `status` to `Completed`, `Failed`, or `Interrupted`. Set `usage` from the accumulated `TurnUsage` delta.
  4. `TurnMetadata` is serialized as part of `turn_started` and `turn_completed`/`turn_failed`/`turn_interrupted` durable records.
- **Postconditions**: Replay reconstructs the turn with status, timing, model info, and usage.
- **Error Handling**: If model resolution fails before invocation, the turn reaches `Failed` with `model: None` and an error item.
- **Edge Cases**: A `GoalContinuation` turn may have no direct user input. A `Resume` turn carries `resume_of_turn_id`. A turn admitted as `steer` during active execution has kind `Steer`.

### B2. Item Kinds and Content Parts

- **Trigger**: The execution engine creates a logical item during a turn.
- **Preconditions**: The item belongs to a valid turn. Content parts are typed.
- **Algorithm / Flow**:
  1. On `item_started`: assign `ItemId`, set `kind` to one of: `UserInput`, `AssistantText`, `AssistantReasoning`, `ToolCall`, `ToolResult`, `ApprovalRequest`, `QuestionRequest`, `SteerMessage`, `QueueMessage`, `Error`, `ContextSummary`.
  2. Content parts are stored as `Vec<ContentPart>` where `ContentPart` is an enum: `Text(String)`, `ImageRef { artifact_id: String }`, `FileRef { path: PathBuf, artifact_id: Option<String> }`, `ToolCallJson(Value)`, `ToolResultText(String)`, `ProviderMetadata(Value)`.
  3. On `item_completed`: set `final_status` and `completed_at`. Compute a content hash for integrity.
  4. On `item_failed`: set error details, preserve any partial content already sent.
- **Postconditions**: Each item is independently replayable. Content parts are referenceable by index.
- **Error Handling**: An item with no content parts after completion is valid (tool calls with no arguments). An item with unknown content part kind in replay logs a warning and preserves the raw data.

### B3. Mention Validation And Attachment

- **Trigger**: Client submits user input with content parts and a structured `mentions` array, usually after the user selected a result from the `@` fuzzy-search popup.
- **Preconditions**: The client has already treated text after leading `@` as a fuzzy-search keyword, not as a typed provider selector.
- **Algorithm / Flow**:
  1. Treat raw text beginning with `@` as ordinary submitted text unless it is accompanied by a structured mention selected by the client.
  2. Validate each client-supplied `Mention` record:
     - `mention_id`: stable within the submitted item.
     - `kind`: `Skill`, `File`, `Directory`, `McpResource`, `McpTemplate`, `ToolOrConnector`, `Session`, `Turn`, `Transcript`, or `Image`.
     - `display_text`: the text rendered in the composer or transcript.
     - `target`: canonical target reference, such as a workspace-relative path, skill id, MCP resource id, or artifact id.
     - `source_range`: byte or grapheme range occupied by the displayed mention token in the submitted text.
     - `resolution_status`: `Resolved`, `Unresolved`, `Stale`, `PermissionBlocked`, or `Ambiguous`.
     - `visibility`: `Visible` or `Hidden`.
  3. Verify that `source_range` is valid for the submitted text and does not split a Unicode scalar or grapheme cluster.
  4. Preserve unresolved, stale, permission-blocked, and ambiguous mentions rather than dropping them; context assembly decides whether they can contribute model context.
  5. Attach validated mentions to the user input item's `mentions` field.
  6. For pasted images or attachments, store the binary/media reference as a `ContentPart` and optionally attach a mention that records how that artifact was resolved or referenced.
- **Postconditions**: Each mention is traceable to client selection state, source text range, and target entity. No server-side parser requires syntax such as `@skill:`, `@file:`, or `@mcp:`.
- **Error Handling**: Invalid mention objects produce field-level validation errors for the submitted item. Raw `@` text without a structured mention is accepted as literal text.

### B4. Plan State Serialization

- **Trigger**: The plan tool creates or updates a plan.
- **Preconditions**: The plan tool has been invoked and produced a result.
- **Algorithm / Flow**:
  1. Serialize the plan as a `PlanRecord` with: `plan_id`, `session_id`, `created_turn_id`, `updated_turn_id`, `objective` (string), `status` (Active, Completed, Blocked, Abandoned, Superseded), `items` (Vec of `PlanItem`).
  2. Each `PlanItem` has: `plan_item_id`, `text`, `status` (Pending, InProgress, Completed, Blocked, Canceled), `details` (optional string), `parent_item_id` (optional), `parallel_group_id` (optional), `source_turn_id`, `updated_at`.
  3. Append `plan_created` on first plan creation. Append `plan_updated` on subsequent mutations. The record includes changed item IDs for efficient client patching.
- **Postconditions**: Replay projects at most one active plan per session by default. Historical plans remain auditable.
- **Error Handling**: Plan items with unknown status values on replay are treated as Pending.

### B5. Goal State Serialization

- **Trigger**: Goal state changes (create, replace, pause, resume, complete, block, cancel, clear, budget update).
- **Preconditions**: A goal mutation has been accepted by the goal system.
- **Algorithm / Flow**:
  1. Append the appropriate durable record: `goal_created`, `goal_replaced`, `goal_status_changed`, `goal_budget_accounted`, `goal_progress_recorded`, `goal_context_snapshot_recorded`, `goal_cleared`.
  2. Each goal record carries: `goal_id`, `session_id`, `objective`, `status`, `token_budget`, `time_budget_seconds`, `turn_budget`, `tokens_used`, `time_used_seconds`, `turns_used`, `progress_summary`, `blocker_summary`, `verification_summary`, timestamps.
  3. Replay projects at most one non-terminal goal. Terminal and cleared goals are retained for audit.
- **Postconditions**: The goal can be fully reconstructed from durable records. Budget accounting is cumulative across appended records.

### B6. Workspace Change Tracking

- **Trigger**: Structured mutating tools (`write`, `apply_patch`) complete, or a turn completes with accumulated file changes.
- **Preconditions**: The turn has a `WorkspaceChangeSet` accumulator. The tool reported file changes.
- **Algorithm / Flow**:
  1. For each file changed by a structured tool, create a `FileChange` record: `file_change_id`, `turn_id`, `tool_call_id`, `tool_name`, `path`, `change_kind` (Create, Modify, Delete, Rename, ModeChange), `pre_state_ref`, `pre_state_hash`, `post_state_ref`, `post_state_hash`, `inverse_ref`, `display_diff_hunk_ref`, `attribution_confidence`.
  2. Accumulate into the turn's `WorkspaceChangeSet`: `change_set_id`, `session_id`, `turn_id`, `checkpoint_id`, `structured_tool_coverage`, `shell_change_coverage`, `file_change_refs`, `display_diff_ref`, `restore_data_ref`, `change_set_status`.
  3. At turn start (before first mutating tool), optionally create a `TurnWorkspaceCheckpoint` with the pre-turn baseline.
  4. Append `turn_workspace_change_recorded` on each change, or aggregate at turn completion.
- **Postconditions**: Each file change is attributable to a specific tool call and turn. Restoration can use inverse records or pre/post state snapshots.

### B7. Immediate Previous Message Editing

- **Trigger**: Client sends `message/editPrevious` for the immediately preceding eligible user-authored message.
- **Preconditions**: The target message is the current branch's immediately preceding eligible user message. The edit is from an authorized client.
- **Algorithm / Flow**:
  1. Follow the eligibility, queued edit, superseded-turn edit, restore ordering, and replacement-turn rules in `L3-BEH-CORE-012`.
  2. Preserve the original message and turn records.
  3. Project the replacement message and replacement turn as the current branch after edit acceptance.
- **Postconditions**: The original message and turn remain durable. A replacement branch is projected. The superseded turn is marked accordingly.
- **Error Handling**: Use `L3-BEH-CORE-012` error codes for stale target, active running turn, and older historical message edits.

### B8. Workspace Restoration for Superseded Turns

- **Trigger**: `message/editPrevious` is accepted for a completed/failed/interrupted turn.
- **Preconditions**: The superseded turn has a `WorkspaceChangeSet`. The workspace is accessible.
- **Algorithm / Flow**:
  1. Use the candidate selection, safe predicate, per-file restore actions, checkpoint handling, and record ordering defined by `L3-BEH-CORE-012`.
  2. Append `turn_workspace_restore_started` before file mutation attempts.
  3. Append `turn_workspace_restore_completed` with per-file outcomes after restore attempts complete.
  4. Emit `workspace_restore_started` and `workspace_restore_completed` protocol events from those durable records.
- **Postconditions**: Files that were unchanged post-turn are restored. Diverged files are preserved. Side effects (API calls, network requests) are not undone.
- **Error Handling**: Restore failure for a file records `failed`. Git checkpoint unavailable falls back to inverse records. No reliable restore data records `unsupported`.

## Traceability

| L2 Source | Relationship |
|---|---|
| L2-DES-CONV-001 | specified-by |
| L2-DES-AGENT-001 | specified-by |
| L2-DES-APP-003 | specified-by |
| L3-BEH-CORE-012 | related-to |

## Implementation Placement Guidance

- Use `serde` tagged enums for item kinds and content part types to enable forward compatibility.
- Existing protocol data types may be reused when they are pure DTOs and match this L3 contract. Do not extend a client DTO into a server persistence model if doing so would leak server-only fields or weaken replay semantics.
- `PlanItem` and `Goal` types live in the core crate as they are the canonical data model; the protocol crate re-exports or references them for wire serialization.

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-27 | Assistant | Initial | Initial turn, item, plan, goal, workspace change, and edit data structure behavior. |
| 2 | 2026-05-27 | Assistant | Correction | Delegated immediate-message-edit and workspace-restore details to L3-BEH-CORE-012 and aligned durable restore record names. |
| 3 | 2026-05-27 | Assistant | Correction | Replaced typed `@skill:`/`@file:` mention parsing with structured mention validation produced by direct-keyword fuzzy search selection. |

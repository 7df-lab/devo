---
artifact_id: L3-BEH-SERVER-002
revision: 2
status: Draft
active_baseline: no
---

# L3-BEH-SERVER-002 — Interrupt, Resume, and Active Work Inspection

## Purpose

Define the concrete behavior for interrupting active execution, inspecting active work, resuming interrupted turns, and crash recovery as specified by L2-DES-AGENT-002.

## Source Design

L2-DES-AGENT-002 (Interrupt And Resume Control), L2-DES-AGENT-001 (Execution Engine), L2-DES-APP-003 (Client Server Protocol), L2-DES-CONV-001 (Session JSONL Data Model)

## Behavior Specification

### B1. Interrupt Request Handling

- **Trigger**: Client sends `turn/interrupt` for an active turn.
- **Preconditions**: The session exists. The target turn is active or recently active.
- **Algorithm / Flow**:
  1. Validate session and turn. If turn is not active: return idempotent success with existing terminal status, or `NoActiveTurn` error.
  2. Append `turn_interrupt_requested` durable record: `interrupt_id`, `session_id`, `turn_id`, `requested_by_client_id`, `target_kind` (model_invocation, tool_call, background_process, approval_wait, entire_turn), `target_id` (specific tool/invocation/approval id), `interrupt_mode`, `reason`, `requested_at`, `status: requested`.
  3. If the durable append fails, do not signal the runtime token. Return a structured persistence error.
  4. Signal the interrupt token for the turn. This is a `tokio::sync::CancellationToken` associated with the active turn.
  5. The execution engine checks the token at phase boundaries and cooperative yield points.
  6. Return the acceptance response immediately: `turn_id`, `interrupt_id`, `interrupt_state`, `cleanup_state`, `latest_sequence`.
  7. Do NOT wait for cleanup to finish in the response path.
- **Postconditions**: The interrupt token is signaled. The turn will transition to interrupted at the next yield point.
- **Error Handling**: Turn not found → `TurnNotFound`. Turn already terminal → return idempotent result with current status. Session not found → `SessionNotFound`.

### B2. Interrupt by Target Kind

- **Trigger**: The execution engine checks the interrupt token during active work.
- **Preconditions**: `CancellationToken` is signaled. The engine is in a specific phase.
- **Algorithm / Flow**:
  Per phase interrupt behavior:
  - **ContextAssembly**: Abort assembly. Mark turn `Interrupted`. No partial model output to record.
  - **ModelInvocation**: Cancel the provider HTTP request/stream. Drain remaining stream events into buffer. Flush partial content: coalesce received deltas into `item_content_appended`, mark the item as `item_failed` (interrupted). Persist partial usage if available.
  - **ToolDispatch**: Send cancellation to running tool handlers where supported. Read-only tools may complete before cancellation takes effect — accept their results. Mutating tools report partial failure or complete atomically. Mark tool calls that could not be cancelled as `interrupted` or `completed_before_interrupt`.
  - **WaitingForApproval/Question**: Resolve the wait as interrupted. Mark pending approval/question as `canceled`. Broadcast resolution.
  - **BackgroundProcessRunning**: Keep process visible. Do not terminate unless explicitly targeted. The process remains in the background process registry.
  - **Finalization**: If terminal status already persisted, treat as stale (return existing status).
- **Postconditions**: Interrupted phase state is clean. Partial work is preserved. Turn transitions to `Interrupted` terminal status.
- **Edge Cases**: Provider stream cannot be cancelled (network timeout) → mark invocation as `failed_to_cancel`, stop forwarding output. Tool cancellation not supported → let tool complete, but mark turn interrupted after.

### B3. Background Process Stop

- **Trigger**: Client sends `backgroundProcess/stop` for a tracked background process.
- **Preconditions**: The process was registered by a tool execution. `process_id` is valid.
- **Algorithm / Flow**:
  1. Look up the process in the background process registry.
  2. Send termination signal: SIGTERM to the process group (Unix) or `TerminateProcess` (Windows).
  3. Wait for process exit up to a configurable deadline (default 5 seconds).
  4. If process exits: record `stopped` state with exit code.
  5. If deadline exceeded: send SIGKILL (Unix) / `TerminateProcess` force (Windows). Record `force_stopped`.
  6. Append `background_process_updated` durable record and emit the `background_process_updated` server-client event with final state.
- **Postconditions**: The process is no longer running. Its final output is captured. The stop state is recorded.
- **Error Handling**: Process not found → `NotFound`. Process already exited → return existing terminal state. Stop permission denied → `PolicyDenied`.

### B4. Active Work Inspection

- **Trigger**: Client sends `execution/inspect`.
- **Preconditions**: A session is active. The client is authorized.
- **Algorithm / Flow**:
  1. Read the current turn execution state.
  2. Build the safe projection:
     - `session_id`, `active_turn_id`, `turn_status`, `turn_phase`.
     - `active_invocation_id`: the current model invocation (if any).
     - `running_tool_calls`: list of tool calls with `tool_name`, `status`, `started_at`, `approval_state`, `progress` (without secret output).
     - `pending_approvals`: list of pending approval prompts with `approval_id`, `summary`, `expires_at`.
     - `pending_questions`: list of pending question prompts.
     - `background_processes`: list of tracked processes with `process_id`, `command_label`, `status`, `runtime`.
     - `workspace_change_set_status`: summary of file changes so far.
     - `last_event_sequence`.
  3. Redact sensitive data: no plaintext secrets, no full command output unless marked safe for display.
- **Postconditions**: Client has enough information to show the user what's running and let them decide what to stop.

### B5. Resume Turn

- **Trigger**: Client sends `turn/resume` for an interrupted or recoverable turn.
- **Preconditions**: The target turn is `Interrupted` or otherwise recoverable. The session is loaded. No conflicting active turn.
- **Algorithm / Flow**:
  1. Validate the target turn is in a recoverable state. Reject if not.
  2. Check workspace availability. If workspace is unavailable and user hasn't accepted degraded mode → reject or warn.
  3. Check that required context records are available. If compacted or missing → reject or warn.
  4. Create a new `TurnId` for the resume turn. Set `resume_of_turn_id` to the interrupted turn.
  5. Build resume context:
     - The original user request from the interrupted turn.
     - Partial assistant output (coalesced from durable records).
     - Completed tool calls and their results.
     - File-change summary from the workspace change set.
     - Background process state (if relevant).
     - User-provided resume instructions (from the resume request).
     - Current session metadata and model selection.
  6. Persist `turn_resume_started` with `interrupted_turn_id`, `resume_turn_id`, `client_resume_id`, `resume_mode`, and optional user-provided resume instructions metadata.
  7. Persist a normal `TurnStartedRecord` for the resume turn with `kind = Resume` and `resume_of_turn_id = interrupted_turn_id`.
  8. Emit the `turn_resumed` server-client event with `interrupted_turn_id`, `resume_turn_id`, and `resume_mode`.
  9. Run the resume turn through the normal execution engine (L3-BEH-CORE-002).
  10. Do NOT reinterpret already-executed model or tool work from the interrupted turn.
- **Postconditions**: A new continuation turn is executing. The interrupted turn remains terminal and auditable.
- **Error Handling**: Turn not in recoverable state → `TurnAlreadyRunning` or `ExpectedTurnMismatch`. Missing context → `ContextLimitExceeded` or warning. Workspace unavailable → reject with clear reason.

### B6. Crash Recovery on Server Restart

- **Trigger**: Server starts and loads sessions.
- **Preconditions**: A session JSONL has an unterminated turn (no `turn_completed`, `turn_failed`, or `turn_interrupted`).
- **Algorithm / Flow**:
  1. During replay (L3-BEH-CORE-001), identify unterminated turns.
  2. For each unterminated turn: append `turn_interrupted` record with `interrupt_mode: crash_recovery`.
  3. For unterminated items: append `item_failed` with error "Session terminated unexpectedly".
  4. For incomplete compaction events: discard and use pre-compaction snapshot.
  5. The turn is now in a recoverable state. Clients may resume it.
- **Postconditions**: All turns have terminal states. The session is consistent.

### B7. Durable Records and Replay Hooks

- **Trigger**: Interrupt request, background process state change, resume admission, or crash recovery.
- **Preconditions**: Session store is writable except during read-only inspection.
- **Algorithm / Flow**:
  1. `turn_interrupt_requested` is appended before the runtime cancellation token is signaled.
  2. `TurnInterruptedRecord` is appended when the turn reaches interrupted terminal status. It should include `interrupt_id` when interruption was user-requested and `interrupt_mode = crash_recovery` when recovery created it.
  3. `background_process_updated` is appended whenever a tracked process starts, exits, detaches, stop is requested, stop succeeds, force-stop succeeds, or stop fails.
  4. `turn_resume_started` is appended when the server accepts a resume request and allocates a `resume_turn_id`.
  5. The resume turn also uses `TurnStartedRecord` with `kind = Resume`; `resume_of_turn_id` links to the interrupted turn.
  6. Replay applies interrupt and resume-started records as audit/provenance; terminal turn records remain canonical turn status.
- **Postconditions**: Durable replay can explain who interrupted work, what cleanup occurred, what processes remained visible, and which turn resumed prior work.

Durable record schemas:

| Record | Required Fields | Purpose |
|---|---|---|
| `turn_interrupt_requested` | `schema_version`, `interrupt_id`, `session_id`, `turn_id`, `requested_by_client_id`, `target_kind`, `target_id`, `interrupt_mode`, `reason`, `requested_at`, `status` | Audit and provenance for user/server interrupt request. |
| `turn_resume_started` | `schema_version`, `session_id`, `interrupted_turn_id`, `resume_turn_id`, `client_resume_id`, `resume_mode`, `resume_instruction_digest`, `requested_by_client_id`, `started_at` | Audit and provenance for accepted resume request before normal resume turn admission. |
| `background_process_updated` | `schema_version`, `process_id`, `session_id`, `turn_id`, `tool_call_id`, `command_label`, `status`, `runtime_ms`, `recent_output_ref`, `stop_state`, `updated_at` | Rebuild tracked background process projection. |

### B8. Required Tests

- `turn/interrupt` appends `turn_interrupt_requested` before signaling cancellation.
- Failed durable append prevents runtime cancellation and returns a structured error.
- Interrupt response returns promptly while cleanup is still pending.
- Provider interruption preserves partial assistant/reasoning content already accepted.
- Tool cancellation preserves completed tool results and terminal states for interrupted tools.
- `backgroundProcess/stop` records stopped, force-stopped, already-exited, and failed-to-stop outcomes.
- `execution/inspect` redacts secrets and does not expose unbounded command output.
- `turn/resume` writes `turn_resume_started` and then `TurnStarted(kind = Resume, resume_of_turn_id = ...)`.
- Crash recovery appends terminal interrupted/item-failed records for unterminated turns and items.
- Replay rebuilds background process projection and linked resume turns.

## Traceability

| L2 Source | Relationship |
|---|---|
| L2-DES-AGENT-002 | specified-by |
| L2-DES-AGENT-001 | specified-by |
| L2-DES-APP-003 | specified-by |
| L2-DES-CONV-001 | specified-by |

## Implementation Notes

- Use `tokio::sync::CancellationToken` for interrupt signaling — checked at cooperative yield points in the execution engine.
- Background process registry is a `HashMap<ProcessId, ProcessHandle>` behind an `Arc<RwLock<>>`.
- The `execution/inspect` projection must never include raw API keys, unredacted tool output, or credential material.

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-27 | Assistant | Initial | Initial interrupt, resume, active work inspection, and crash recovery behavior. |
| 2 | 2026-05-27 | Assistant | Correction | Aligned interrupt, resume, and background process durable records with the L2 JSONL record vocabulary and added replay hooks/tests. |

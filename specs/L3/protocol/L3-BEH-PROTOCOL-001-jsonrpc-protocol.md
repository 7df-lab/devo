---
artifact_id: L3-BEH-PROTOCOL-001
revision: 3
status: Draft
active_baseline: no
---

# L3-BEH-PROTOCOL-001 — JSON-RPC Client-Server Protocol

## Purpose

Define the concrete behavior for JSON-RPC 2.0 request/response handling, server notification delivery, WebSocket transport binding, sequence management, and cross-client broadcast as specified by L2-DES-APP-003.

## Source Design

L2-DES-APP-003 (Client Server Protocol)

## Behavior Specification

### B0. Method Naming And Existing Event Compatibility

JSON-RPC `method` names use `/` path separators. Dot-form method names are not part of the protocol contract.

Implementation rules:

1. Request handlers register slash names such as `server/initialize`, `session/subscribe`, `turn/submit`, and `config/update`.
2. Server notification names should reuse `ServerEvent::method_name()` from `crates/protocol/src/event.rs` when the notification already exists there.
3. Existing concrete notification names include `session/started`, `session/title/updated`, `session/compaction/started`, `session/compaction/completed`, `session/compaction/failed`, `session/status/changed`, `session/archived`, `session/unarchived`, `session/closed`, `turn/started`, `turn/completed`, `turn/interrupted`, `turn/failed`, `turn/plan/updated`, `turn/diff/updated`, `turn/usage/updated`, `inputQueue/updated`, `steer/accepted`, `item/started`, `item/completed`, `item/agentMessage/delta`, `item/reasoning/summaryTextDelta`, `item/reasoning/textDelta`, `item/commandExecution/outputDelta`, `item/fileChange/outputDelta`, `item/plan/delta`, and `serverRequest/resolved`.
4. New notification names that are not yet represented in code should follow the same slash style.
5. Conceptual event-kind names may remain inside payload fields or projections, but they are not the JSON-RPC envelope method name.

### B1. JSON-RPC Envelope Validation

- **Trigger**: A WebSocket message arrives from a client.
- **Preconditions**: The WebSocket connection is open. The client has completed `server/initialize`.
- **Algorithm / Flow**:
  1. Parse the incoming text as a JSON-RPC 2.0 message.
  2. Validate that `jsonrpc: "2.0"` is present.
  3. If the message has an `id` field, treat as a request: route to the method handler, produce a response with the same `id`.
  4. If the message has no `id` field, treat as a notification: route to the method handler, produce no response.
  5. If the message is invalid JSON, return JSON-RPC parse error (code -32700).
  6. If the method is unknown, return JSON-RPC method not found (code -32601).
- **Postconditions**: Every request gets exactly one response. Notifications are fire-and-forget.
- **Error Handling**: Invalid JSON → `{"jsonrpc":"2.0","error":{"code":-32700,"message":"Parse error"},"id":null}`. Unknown method → error code -32601. Invalid params → error code -32602 with `data` containing field-level errors.
- **Edge Cases**: A message with `id: null` is treated as a notification per JSON-RPC 2.0 spec. Batch requests (array of request objects) are not supported in v1.

### B2. Client Initialization

- **Trigger**: A new WebSocket connection is established and the client sends `server/initialize`.
- **Preconditions**: The server is running and accepting connections.
- **Algorithm / Flow**:
  1. Parse params: `client_id` (string), `client_kind` (tui, desktop, ide, browser), `protocol_version` (semver string), `auth_token` (string, optional), `client_capabilities` (object), `workspace_root` (path, optional).
  2. Verify `protocol_version` is compatible (same major version).
  3. Authenticate the client if `auth_token` is required and present. Reject with auth error if invalid.
  4. Register the client connection in the server's client table: assign an internal `connection_id`, store `client_kind`, `client_capabilities`.
  5. Return: `server_id`, `server_version`, `protocol_version`, `server_capabilities` (supported methods, event categories), `latest_sequence` (0 if no active subscriptions).
  6. Subsequent requests from this connection are processed in the context of this registered client.
- **Postconditions**: The client is registered and can subscribe to sessions. Uninitialized connections that send other methods receive `NotInitialized` error.
- **Error Handling**: Incompatible protocol version → error with `data.supported_versions`. Invalid auth → error with `data.auth_error`. Missing required capability → warning in response but connection proceeds in degraded mode.

### B3. Session Subscription and Event Delivery

- **Trigger**: Client sends `session/subscribe` after initialization.
- **Preconditions**: The session exists. The client is authorized to access this session.
- **Algorithm / Flow**:
  1. Parse params: `session_id`, `from_sequence` (optional u64), `event_filter` (optional set of event kinds), `projection` (optional projection specifier).
  2. Load the session from durable storage or in-memory cache. Verify the client is authorized.
  3. Create a `Subscription` with a unique `subscription_id`.
  4. If `from_sequence` is provided and events after that sequence are available in the server's event buffer:
     - Replay missed events to the client in order.
  5. If `from_sequence` is too old or unknown:
     - Send a `session_loaded` snapshot event with the current session projection and `latest_sequence`.
  6. Register the subscription to receive future session-scoped and turn-scoped concrete notifications, such as `session/started`, `turn/started`, `item/started`, and `item/agentMessage/delta`.
  7. Return: `subscription_id`, optional `session_snapshot`, `next_sequence`.
- **Postconditions**: The client receives ordered events for the session. Events are filtered by `event_filter` if specified.
- **Error Handling**: Session not found → `SessionNotFound`. Client not authorized → `PolicyDenied`. Subscription limit exceeded → error with `data.max_subscriptions`.

### B4. Monotonic Sequence Assignment

- **Trigger**: Any server event is generated for a session.
- **Preconditions**: The session has a `session_sequence` counter starting at 0.
- **Algorithm / Flow**:
  1. Atomically increment the session's sequence counter.
  2. Assign the new value to the event's `seq` or `sequence` field.
  3. Buffer the event in the session's event ring buffer (configurable capacity, default 1000 events).
  4. Push the event to all subscribed client WebSocket connections for that session.
- **Postconditions**: Every event has a unique, monotonically increasing sequence number within its session.
- **Error Handling**: If the event buffer is full, evict the oldest event. Clients requesting `from_sequence` before the oldest buffered event receive a `session_loaded` snapshot instead.

### B5. Server Notification Broadcasting

- **Trigger**: A server notification (listed in L2-DES-APP-003 Server Notifications table) is generated.
- **Preconditions**: At least one client is subscribed to the relevant session or is connected for server-level notifications.
- **Algorithm / Flow**:
  1. Construct the notification envelope: `{"jsonrpc": "2.0", "method": "<method>", "params": {...}}`.
  2. Determine the target client set: session-scoped and turn-scoped concrete events go to session subscribers; server-level events such as `server/status/changed` and `config/changed` go to all connected clients who expressed interest.
  3. Send the notification on each target client's WebSocket.
  4. Do not wait for client acknowledgement. Notifications are fire-and-forget from the server's perspective.
- **Postconditions**: All subscribed clients receive the notification. Disconnected clients miss notifications until re-subscribe.
- **Error Handling**: WebSocket send failure for one client does not block delivery to other clients. A client that repeatedly fails to receive is disconnected.

### B6. Turn Submission and Admission

- **Trigger**: Client sends `turn/submit`.
- **Preconditions**: The client is subscribed to the session, or is creating a new session.
- **Algorithm / Flow**:
  1. Parse params: `session_id` (or `new_session` flag), `submission_mode` (Normal, Steer, Queue), `active_turn_id` (required for Steer), `content_parts`, `mentions`, `client_message_id`, optional `mode_overrides`.
  2. If new session: validate `workspace_root`, create session, return `session_id`.
  3. If existing session:
     a. If no active turn and `submission_mode` is Normal: admit as new turn.
     b. If active turn and `submission_mode` is Steer: record steer item on active turn.
     c. If active turn and `submission_mode` is Queue: create queue item.
     d. If active turn and `submission_mode` is Normal: reject with `TurnAlreadyRunning` (suggest steer or queue).
  4. Check idempotency: if `client_message_id` was already processed, return the original canonical IDs.
  5. Persist the accepted input (user item) before returning.
  6. Return: `session_id`, `turn_id` (or `queue_item_id` or `steer_item_id`), `accepted`, `classification`, `latest_sequence`.
- **Postconditions**: The accepted input is durable. The execution engine will pick up the turn if applicable.
- **Error Handling**: `TurnAlreadyRunning` for Normal submission during active turn. `NoActiveTurn` for Steer without active turn. `ActiveTurnNotSteerable` when target turn is in finalization. `EmptyInput` when content_parts is empty.

### B7. Request Idempotency

- **Trigger**: Client retries a request with the same `client_message_id` or request `id`.
- **Preconditions**: The original request was processed and its result is still available.
- **Algorithm / Flow**:
  1. On receiving a request, check the `id` or `client_message_id` against a recently-processed request cache (LRU, capacity 1000 per connection, TTL 60 seconds).
  2. If found and the request params match: return the cached response.
  3. If found but params differ: return error "id already used with different params".
  4. If not found: process normally and cache the result.
- **Postconditions**: Duplicate requests are safe to retry. No duplicate turns, items, or side effects from retries.

### B8. Approval Resolution Race Handling

- **Trigger**: Multiple clients attempt to resolve the same approval request.
- **Preconditions**: An `approval/requested` notification was broadcast. The approval is still pending.
- **Algorithm / Flow**:
  1. First `approval/respond` received: validate `approval_id` and `expected_turn_id`, accept the decision, broadcast `approval_resolved`, resume or deny the tool call.
  2. Subsequent `approval/respond` for the same `approval_id`: return error `already_resolved` with the final decision in `data`.
  3. Expired approval (timeout): auto-resolve as denied, broadcast `approval_resolved`.
- **Postconditions**: Each approval is resolved exactly once.

### B9. Session Fork Request Handling

- **Trigger**: Client sends `session/fork`.
- **Preconditions**: Client is initialized and authorized to read the parent session.
- **Params**:
  - `parent_session_id`: required session id.
  - `fork_turn_id`: required turn id.
  - `workspace_root`: optional absolute path. If omitted, core resolves from the parent fork point.
  - `fork_label`: optional display label.
  - `client_fork_id`: optional idempotency key.
- **Algorithm / Flow**:
  1. Validate parameter types.
  2. Check idempotency using `client_fork_id` when provided.
  3. Delegate fork admission, inherited segment construction, and child session creation to `L3-BEH-CORE-011`.
  4. On success, return `session_id`, `parent_session_id`, `fork_turn_id`, `inherited_segment_id`, `session_snapshot`, and `latest_sequence`.
  5. Broadcast a concrete session notification, such as `session/started` with a child session snapshot, or a fork-specific slash notification if later protocol revisions add one.
- **Errors**:
  - `ParentSessionNotFound`
  - `ForkTurnNotFound`
  - `ForkTurnNotStable`
  - `PermissionDenied`
  - `WorkspaceUnavailable`
  - `InheritedSegmentWriteFailed`
- **Postconditions**: The client receives a child session snapshot only after the inherited segment and child fork records are durable.

### B10. Session Delete With Fork Retention

- **Trigger**: Client sends `session/delete`.
- **Preconditions**: Client is initialized and authorized to delete or request deletion of the session.
- **Params**:
  - `session_id`: required session id.
  - `delete_mode`: required, `tombstone`, `archive_then_delete`, or `hard_delete` where supported.
  - `fork_policy`: required, `preserve_forks`, `materialize_then_delete`, `block_if_forks`, or `cascade_delete` where supported.
  - `confirm_token`: required when a prior preflight response requested confirmation.
  - `client_delete_id`: optional idempotency key.
- **Algorithm / Flow**:
  1. Validate parameter types and supported policy combinations.
  2. Run core deletion preflight from `L3-BEH-CORE-011`.
  3. If confirmation is required and missing, return `accepted = false`, `delete_state = requires_confirmation`, `affected_forks`, `inherited_segment_actions`, `retained_records`, `confirm_token`, and `latest_sequence`.
  4. If confirmation is present, re-run preflight and verify the token still matches current affected-fork state.
  5. Delegate deletion commit to core.
  6. Return `accepted`, `session_id`, `delete_state`, `affected_forks`, `inherited_segment_actions`, `retained_records`, and `latest_sequence`.
  7. Broadcast `session_deleted` only after durable deletion/tombstone records and required inherited segment materialization are complete.
- **Errors**:
  - `SessionNotFound`
  - `PermissionDenied`
  - `ForkRetentionRequired`
  - `InvalidConfirmToken`
  - `UnsupportedDeletePolicy`
  - `InheritedSegmentMaterializationFailed`
- **Postconditions**: A surviving fork remains replayable, or deletion is blocked/cascaded according to explicit user policy.

### B11. Immediate Previous Message Edit Request Handling

- **Trigger**: Client sends `message/editPrevious`.
- **Preconditions**: Client is initialized and authorized for the session.
- **Params**:
  - `session_id`: required session id.
  - `expected_target_message_id`: required item id.
  - `edited_content_parts`: required non-empty content part list.
  - `edited_mentions`: required mention list, may be empty.
  - `client_edit_id`: required idempotency key.
  - `edit_mode`: optional. Initial supported value is `default`.
  - `workspace_restore_policy`: optional, default `default_safe`.
- **Algorithm / Flow**:
  1. Validate content parts and mention schema using the same validation as `turn/submit`.
  2. Check idempotency by `client_edit_id`.
  3. Delegate eligibility, restore planning, restore execution, supersession, and replacement turn admission to `L3-BEH-CORE-012`.
  4. Return `accepted`, `edit_id`, `target_message_id`, `replacement_message_id`, optional `superseded_turn_id`, `workspace_restore_state`, optional `new_turn_id` or `queue_item_id`, `edit_state`, and `latest_sequence`.
  5. Broadcast canonical events in durable sequence order:
     - `message_edit_recorded`,
     - `workspace_restore_started` and `workspace_restore_completed` when restoration runs,
     - `turn_superseded` when a prior turn is replaced,
     - normal replacement turn events when execution starts.
- **Errors**:
  - `SessionNotFound`
  - `ExpectedTargetMessageMismatch`
  - `OlderMessageRequiresFork`
  - `ActiveTurnEditRejected`
  - `InvalidContentParts`
  - `InvalidMentions`
  - `WorkspaceRestoreFailedToStart`
- **Postconditions**: Clients do not mutate local history themselves; they reconcile against canonical server events.

## Traceability

| L2 Source | Relationship |
|---|---|
| L2-DES-APP-003 | specified-by |
| L2-DES-CONV-001 | specified-by |

## Implementation Placement Guidance

- Protocol envelope DTOs such as `ClientRequest`, `SuccessResponse`, and `ErrorResponse` belong in the protocol crate. Existing names may be reused if their fields match this L3 contract.
- `ProtocolErrorCode` must include the errors required by this document, including `AlreadyResolved`; existing enums should be adjusted rather than treated as complete by default.
- Sequence counter should use `AtomicU64` per session for lock-free increment.
- The event ring buffer should use a pre-allocated `VecDeque` with a max capacity.

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-27 | Assistant | Initial | Initial JSON-RPC envelope, initialization, subscription, sequencing, turn submission, idempotency, and approval race behavior. |
| 2 | 2026-05-27 | Assistant | Correction | Added concrete request handling for session fork, session delete with fork retention, and immediate previous message editing. |
| 3 | 2026-05-27 | Assistant | Correction | Changed JSON-RPC method names to slash-separated names and required reuse of existing `ServerEvent::method_name()` notification names where available. |

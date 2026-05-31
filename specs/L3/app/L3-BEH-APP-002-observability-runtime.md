---
artifact_id: L3-BEH-APP-002
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-27
---

# L3-BEH-APP-002 - Observability Runtime

## Purpose

Define implementation behavior for structured logs, active diagnostics, trace-mode diagnostic records, correlation identifiers, redaction, retention, and optional telemetry.

## Source Design

- `L2-DES-APP-004` defines the cross-system observability architecture.
- `L2-DES-LLM-003` defines model usage and model stream observability.
- `L2-DES-APP-003` defines client/server protocol projections.
- `L2-DES-AGENT-001` defines execution phases to observe.
- `L2-DES-TOOL-001` defines tool lifecycle observability points.

## Core Types

```rust
pub struct ObservabilityConfig {
    pub log_level: LogLevel,
    pub trace_mode: TraceMode,
    pub trace_content_mode: TraceContentMode,
    pub telemetry_enabled: bool,
    pub log_retention: RetentionPolicy,
    pub trace_retention: RetentionPolicy,
}

pub struct CorrelationIds {
    pub session_id: Option<SessionId>,
    pub turn_id: Option<TurnId>,
    pub item_id: Option<ItemId>,
    pub invocation_id: Option<InvocationId>,
    pub tool_call_id: Option<ToolCallId>,
    pub client_id: Option<ClientId>,
    pub request_id: Option<String>,
    pub trace_id: Option<TraceId>,
}

pub struct DiagnosticEvent {
    pub timestamp: DateTime<Utc>,
    pub component: ObservabilityComponent,
    pub event_name: String,
    pub phase: String,
    pub status: DiagnosticStatus,
    pub message: String,
    pub ids: CorrelationIds,
    pub fields: RedactedFields,
    pub recovery_hint: Option<RecoveryHint>,
}

pub struct ActiveDiagnostics {
    pub server_status: ServerStatus,
    pub active_session_id: Option<SessionId>,
    pub active_turn_id: Option<TurnId>,
    pub turn_phase: Option<TurnPhase>,
    pub waiting_reason: Option<WaitingReason>,
    pub current_model: Option<String>,
    pub current_provider: Option<String>,
    pub active_invocation_id: Option<InvocationId>,
    pub running_tool_calls: Vec<ToolCallSummary>,
    pub pending_approvals: Vec<ApprovalSummary>,
    pub pending_questions: Vec<QuestionSummary>,
    pub usage_summary: Option<UsageProjection>,
    pub context_pressure: Option<ContextPressureProjection>,
    pub last_error: Option<DiagnosticError>,
}
```

`DiagnosticEvent` is the common internal input. Structured logs, client diagnostics, trace records, and telemetry are separate projections of that input.

## B1. Initialize Observability

- **Trigger**: Server startup, client startup, or configuration reload.
- **Preconditions**: Effective configuration is available.
- **Algorithm / Flow**:
  1. Read `[logging]` and `[telemetry]` from `EffectiveConfig`.
  2. Apply CLI or environment overrides only when a later CLI design explicitly defines them.
  3. Initialize local log writer.
  4. Initialize trace writer only when trace mode is enabled.
  5. Initialize telemetry exporter only when telemetry is enabled.
  6. Publish a startup `server/status/changed` diagnostic event.
- **Postconditions**: Observability sinks are ready before accepting turns.
- **Errors**: If log initialization fails, continue with stderr diagnostics and emit a startup warning when possible. Telemetry initialization failure must not block local operation.

## B2. Capture Runtime Events

- **Trigger**: A subsystem reaches an observability point.
- **Preconditions**: The subsystem can provide a component, phase, status, and correlation ids.
- **Algorithm / Flow**:
  1. Build a `DiagnosticEvent`.
  2. Apply redaction to structured fields before handing the event to any sink.
  3. Update `ActiveDiagnostics` if the event changes current state.
  4. Write a structured log if the event level is enabled.
  5. Write a trace record when trace mode is enabled and the event is trace-eligible.
  6. Emit safe server-client notifications where the event changes user-visible state.
  7. Enqueue a telemetry projection only if telemetry is enabled and the event is telemetry-eligible.
- **Postconditions**: A single event capture path feeds all observability surfaces without sharing sensitive payloads by default.

Required capture points:

- Client connect, initialize, subscribe, unsubscribe, reconnect, disconnect.
- Turn admission, rejection, start, phase change, completion, failure, interruption, resume.
- Context assembly start, finish, compaction decision, compaction completion, context-pressure update.
- Model resolution, invocation start, stream start, usage received, invocation finish, provider failure.
- Tool validation, permission decision, approval wait, execution start, progress, finish, failure, cancellation.
- Persistence append, flush, replay, recovery, and replay failure.
- Configuration load, validation failure, write start, write finish, write failure, and effective configuration change.

## B3. Structured Log Records

- **Trigger**: `DiagnosticEvent` has level `error`, `warn`, `info`, `debug`, or `trace` enabled by config.
- **Preconditions**: Event has already been redacted.
- **Algorithm / Flow**:
  1. Serialize as one JSON object per line.
  2. Include:
     - `timestamp`
     - `level`
     - `component`
     - `event_name`
     - `phase`
     - `status`
     - `message`
     - `ids`
     - `fields`
     - `redaction_state`
     - `recovery_hint`
  3. Flush according to the log writer policy.
- **Postconditions**: Logs are machine-readable and filterable by `session_id`, `turn_id`, `invocation_id`, or `tool_call_id`.
- **Privacy**: Normal structured logs must not include prompt text, response text, attachment content, tool output content, credential values, or authorization headers.

## B4. Active Diagnostic Projection

- **Trigger**: Current state changes.
- **Preconditions**: The server owns authoritative execution state.
- **Algorithm / Flow**:
  1. Maintain one active diagnostic projection per server and per active session.
  2. Update fields from accepted events only.
  3. Broadcast safe changes through existing protocol notifications:
     - `server/status/changed`
     - `session/status/changed`
     - `turn/usage/updated`
     - `turn/plan/updated`
     - `turn/diff/updated`
     - `turn/failed`
     - `item/started`
     - `item/completed`
     - `item/agentMessage/delta`
     - `item/commandExecution/outputDelta`
     - `item/fileChange/outputDelta`
     - `config/changed`
  4. Include sequence numbers through the protocol layer.
  5. Allow `execution/inspect` or equivalent status requests to return the latest projection.
- **Postconditions**: Clients do not infer server state from partial UI state.

## B5. Trace-Mode Records

- **Trigger**: Trace mode is enabled and a trace-eligible model, tool, protocol, or persistence event occurs.
- **Preconditions**: A `trace_id` is assigned for the operation.
- **Algorithm / Flow**:
  1. Store trace records separately from durable session JSONL.
  2. Use one JSONL trace stream per trace id or invocation id.
  3. Preserve sequence, timestamp, elapsed time, event kind, normalized kind, sizes, and completion state.
  4. Apply `TraceContentMode`:
     - `metadata_only`: omit content, keep sizes and event kinds.
     - `redacted_content`: include redacted content only.
     - `content_ref`: store content in a protected artifact and reference it.
     - `inline_content`: include content only when explicitly enabled for local debugging.
  5. Never write credentials or authorization headers.
- **Postconditions**: Trace mode can explain stream timing and ordering without becoming session source of truth.

## B6. Redaction Pipeline

- **Trigger**: Any event field is about to leave a subsystem boundary.
- **Preconditions**: Raw fields may contain sensitive data.
- **Algorithm / Flow**:
  1. Classify field sensitivity:
     - credential
     - prompt_content
     - response_content
     - tool_output
     - local_path
     - provider_metadata
     - safe_identifier
  2. Apply the most restrictive rule required by the destination sink.
  3. Replace secrets with stable redaction markers such as `<redacted:credential>`.
  4. Preserve enough metadata to show that redaction occurred.
  5. Reject telemetry projection if required fields cannot be safely redacted.
- **Postconditions**: Redaction happens before logs, client events, trace files, or telemetry receive data.

## B7. Optional Telemetry

- **Trigger**: A telemetry-eligible event occurs and telemetry is enabled.
- **Preconditions**: User has opted in through durable configuration.
- **Algorithm / Flow**:
  1. Project the event into aggregate product-health data:
     - feature name
     - latency bucket
     - error category
     - provider class
     - tool category
     - client kind
  2. Exclude prompt text, response text, tool output, credentials, exact local file contents, and plaintext local paths.
  3. Buffer locally with bounded queue size.
  4. Drop oldest telemetry events when the queue is full.
  5. Export asynchronously without blocking agent work.
- **Postconditions**: Local observability remains useful when telemetry is disabled or export fails.

## B8. Retention And Deletion

- **Trigger**: Startup, shutdown, scheduled cleanup, or explicit diagnostic cleanup.
- **Preconditions**: Retention policy is configured.
- **Algorithm / Flow**:
  1. Keep durable session JSONL separate from logs and trace records.
  2. Apply log retention to structured logs.
  3. Apply stricter trace retention to trace records.
  4. Delete trace artifacts without modifying session replay.
  5. Record cleanup errors as warnings.
- **Postconditions**: Diagnostic data can be removed without corrupting sessions.

## B9. Required Tests

- Log records contain required fields and correlation ids.
- Log level changes alter emitted records but not runtime behavior.
- Prompt, response, credential, and tool output content are absent from normal logs.
- Trace `metadata_only` records preserve event order, timing, and sizes while omitting content.
- Client diagnostics are derived from server state and include sequence numbers.
- Telemetry is not emitted when disabled.
- Telemetry payloads exclude content and secrets when enabled.
- Retention cleanup deletes trace files without changing session replay.
- Provider authentication errors produce category, phase, and recovery hint without secret leakage.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| specifies | L2-DES-APP-004 | 1 | specs/L2/app/L2-DES-APP-004-observability-architecture.md | Implements structured logs, active diagnostics, trace mode, redaction, retention, and telemetry boundaries. |
| related-to | L2-DES-LLM-003 | 1 | specs/L2/llm/L2-DES-LLM-003-model-usage-observability.md | Provides the observability sinks used by model usage and stream records. |
| related-to | L2-DES-APP-003 | 2 | specs/L2/app/L2-DES-APP-003-client-server-protocol.md | Emits safe diagnostic projections through the client/server protocol. |
| related-to | L2-DES-AGENT-001 | 1 | specs/L2/agent/L2-DES-AGENT-001-execution-engine.md | Captures execution phase transitions. |
| related-to | L2-DES-TOOL-001 | 1 | specs/L2/tool/L2-DES-TOOL-001-built-in-tool-system.md | Captures tool lifecycle diagnostics. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-27 | Assistant | Initial | Initial L3 observability runtime behavior. |

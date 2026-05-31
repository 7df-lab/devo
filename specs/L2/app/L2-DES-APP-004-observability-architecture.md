---
artifact_id: L2-DES-APP-004
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-23
---

# L2-DES-APP-004 — Observability Architecture

## Purpose

Refine application observability into a technical design for structured logs, diagnostic projections, trace-mode records, correlation identifiers, and optional telemetry across clients, server, model calls, tools, and user interface paths.

## Background / Context

Agent workflows cross multiple components before a user sees a result. A single failure may involve client submission, server turn admission, context assembly, model-provider resolution, provider streaming, tool execution, approval waiting, persistence, or rendering. Observability must expose enough state to identify the failing phase without exposing secrets or forcing users to inspect hidden implementation state.

## Source Requirements

- `L1-REQ-APP-004` requires structured logs, configurable log levels, actionable diagnostics, trace logging for large language model streaming response events, and optional telemetry.
- `L1-REQ-LLM-003` requires model usage and stream observability.
- `L1-REQ-APP-011` requires actionable error recovery.
- `L1-REQ-APP-012` requires privacy and data ownership controls.
- `L1-REQ-AGENT-001` requires a visible execution workflow.
- `L1-REQ-TOOL-002` requires observable tool execution paths.
- `L2-DES-APP-003` defines the client/server protocol projections used by clients.
- `L2-DES-AGENT-001` defines the execution engine phases that must be observable.
- `L2-DES-LLM-003` defines model usage and streaming trace records.
- `L2-DES-TOOL-001` defines tool lifecycle and result summaries.

## Design Requirement

The program should implement observability as a set of related but separate outputs:

1. User-facing diagnostics for current status and troubleshooting.
2. Structured logs for maintainers and local debugging.
3. Trace-mode diagnostic records for high-detail model stream investigation.
4. Optional telemetry events only when the user enables telemetry.

These outputs may be derived from the same runtime events, but they should not expose the same data by default. User-facing diagnostics must be concise and actionable. Structured logs should be machine-readable. Trace records may be more detailed and must be guarded by explicit privacy, secret-handling, and retention controls.

## Observability Planes

### User-Facing Diagnostics

User-facing diagnostics are safe projections that clients may show directly.

Examples:

- Current model and provider status.
- Current turn phase: model output, tool output, approval, question, persistence, or rendering.
- Token usage and context pressure.
- Tool name, status, timing, and result summary.
- Provider error phase and recovery hint.
- Whether a value is measured, estimated, unavailable, or redacted.

User-facing diagnostics should avoid raw prompt text, raw credentials, full tool outputs by default, and provider-native error payloads that may contain secrets.

### Structured Logs

Structured logs are local diagnostic records emitted by the server and clients.

Each structured log record should carry:

- `timestamp`
- `level`: trace, debug, info, warn, or error.
- `component`: client, server, agent, model, provider, tool, persistence, protocol, or UI.
- `event_name`
- `phase`
- `status`
- `message`
- Correlation identifiers where available.
- Structured fields after redaction.
- `redaction_state`
- Optional `recovery_hint`

Structured logs should be useful without relying on free-form text parsing. The `message` field is for humans; structured fields are for filtering and analysis.

### Trace-Mode Records

Trace-mode records are high-detail diagnostic records enabled only by explicit configuration or runtime option.

Trace mode may record model stream event timing, ordering, completion state, event kinds, chunk sizes, normalized deltas, provider error metadata, and usage timing. Content-bearing stream deltas must follow the privacy and retention policy defined by `L2-DES-LLM-003`.

Trace mode must not be required for normal operation.

### Optional Telemetry

Telemetry is outbound diagnostic data. It must be disabled unless the user enables it.

Telemetry events must be redacted and aggregate-oriented. They should not include prompt content, response content, tool output content, API keys, local absolute file contents, or secrets.

## Correlation Model

Observability records should use stable correlation identifiers so a user or maintainer can connect related events across subsystems.

Common identifiers:

- `session_id`
- `turn_id`
- `item_id`
- `invocation_id`
- `tool_call_id`
- `approval_id`
- `question_id`
- `client_id`
- `subscription_id`
- `request_id`
- `trace_id`
- `workspace_change_set_id`

Rules:

- Every model invocation should have an `invocation_id`.
- Every tool call should have a `tool_call_id`.
- Every client request should have a request identifier from JSON-RPC or an explicit idempotency id.
- Logs for a single turn should be filterable by `session_id` and `turn_id`.
- Provider-stream trace records should be filterable by `invocation_id` and `trace_id`.

## Log Levels

The program should define log levels consistently:

| Level | Intended Use | Content Policy |
|---|---|---|
| `error` | Terminal failures and failed operations requiring action. | Redacted structured error data and recovery hints. |
| `warn` | Degraded behavior, retries, skipped restore, unavailable provider fields. | Redacted summaries and phase data. |
| `info` | Important lifecycle transitions. | Safe identifiers, statuses, and summaries. |
| `debug` | Developer diagnostics for state transitions and decisions. | Redacted structured fields; no prompt or response content by default. |
| `trace` | High-detail timing and stream diagnostics. | May include sensitive trace records only when trace mode and retention policy allow it. |

Changing log level should not change core behavior. It only changes what diagnostic records are emitted.

## Diagnostic State Model

The runtime should maintain a current diagnostic projection for active work.

Conceptual active diagnostic fields:

- `server_status`
- `active_session_id`
- `active_turn_id`
- `turn_status`
- `turn_phase`
- `waiting_reason`: model, tool, approval, question, persistence, rendering, or none.
- `current_model`
- `current_provider`
- `active_invocation_id`
- `running_tool_calls`
- `pending_approvals`
- `pending_questions`
- `usage_summary`
- `context_pressure`
- `last_error`
- `recovery_actions`

This projection should be exposed to clients through existing protocol surfaces such as session snapshots, turn events, `execution/inspect`, `usage_updated`, `context_updated`, `tool_call_updated`, and `error_reported`.

## Event Capture Points

The server should emit observability records at the following points:

- Client connection, initialization, subscription, reconnect, and disconnect.
- Turn admission, rejection, start, status change, completion, failure, interruption, and resume.
- Context assembly start, completion, compaction decision, and context-pressure update.
- Model resolution, model invocation start, stream start, stream completion, usage receipt, and provider failure.
- Tool validation, approval wait, execution start, progress update, completion, failure, and cancellation.
- Queue, steer, and message-edit state transitions.
- Persistence write, replay, recovery, and replay failure.
- Configuration changes that affect model, provider, log level, telemetry, or trace mode.

The observability event should identify the phase and the affected object rather than only saying that something failed.

## Client Visibility

Clients should not infer runtime state from incomplete local UI state. The server should provide canonical diagnostic events and snapshots.

Client-visible diagnostic behavior should include:

- Showing whether a turn is waiting for model output, tool output, approval, question, or user input.
- Showing model usage and context pressure when available.
- Showing failed tool name, tool phase, and safe result summary.
- Showing provider error category such as authentication, network, rate limit, unavailable model, invalid request, or unknown.
- Showing recovery actions when known, such as update credentials, choose another model, retry later, reduce context, or inspect logs.

Clients may provide local UI render diagnostics, but server-owned execution state remains authoritative.

## Redaction And Privacy

Observability must apply redaction before data crosses a boundary with lower trust or broader visibility.

Rules:

- API keys and credential material must never be logged in plaintext.
- Secret-looking environment variables, headers, provider request authorization fields, and tool outputs must be redacted.
- Prompt content, response content, and attachment content must not appear in normal logs.
- Trace-mode content logging must be explicitly enabled and governed by retention controls.
- User-facing diagnostics should prefer summaries, statuses, counts, and references over raw content.
- Redacted records should say that redaction occurred instead of silently omitting important context.

## Retention

The program should separate retention policy by data sensitivity:

- User-facing session state follows session persistence rules.
- Structured logs follow configured local log retention.
- Trace-mode records follow stricter retention because they may include content-sensitive stream data.
- Telemetry follows telemetry opt-in and data minimization rules.

Trace records should be easy to delete without corrupting durable session replay. They are diagnostic artifacts, not the source of truth for session state.

## Error Diagnostics

Errors should be classified by phase and recovery path.

Conceptual error fields:

- `error_id`
- `scope`: server, session, turn, invocation, tool, client, or persistence.
- `phase`
- `category`
- `message`
- `recoverable`
- `retryable`
- `retry_after`
- `provider_error_ref` where applicable.
- `tool_call_id` where applicable.
- `recovery_actions`
- `redaction_state`

Examples:

- Provider credentials invalid: category `authentication`, recovery action `update_provider_credentials`.
- Model unavailable: category `model_unavailable`, recovery action `choose_different_model`.
- Tool command timed out: category `tool_timeout`, recovery action `retry_or_interrupt`.
- Persistence write failed: category `persistence_failure`, recovery action `free_disk_space_or_check_permissions`.

## Telemetry Boundary

Telemetry is not required for local observability. Local logs and diagnostics must remain useful when telemetry is disabled.

When telemetry is enabled, outbound telemetry should be limited to product-health events such as:

- Feature usage counts.
- Error categories.
- Latency buckets.
- Provider class, not plaintext provider endpoint if sensitive.
- Tool category, not raw command text.

Telemetry must not include model prompts, model responses, tool outputs, credentials, exact local file contents, or unredacted paths unless a later approved requirement explicitly allows them.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-APP-004 | 1 | specs/L1/L1-REQ-APP-004-observability.md | Defines cross-system observability architecture, log levels, diagnostics, trace mode, privacy, and telemetry boundaries. |
| related-to | L1-REQ-LLM-003 | 1 | specs/L1/L1-REQ-LLM-003-observability.md | Model usage and stream observability are specialized observability concerns. |
| related-to | L1-REQ-APP-011 | 1 | specs/L1/L1-REQ-APP-011-error-recovery.md | Actionable diagnostics drive recovery guidance. |
| related-to | L1-REQ-APP-012 | 1 | specs/L1/L1-REQ-APP-012-privacy-data-ownership.md | Observability records must obey privacy and redaction constraints. |
| related-to | L2-DES-APP-003 | 2 | specs/L2/app/L2-DES-APP-003-client-server-protocol.md | Client/server events and snapshots expose safe diagnostics to clients. |
| related-to | L2-DES-AGENT-001 | 1 | specs/L2/agent/L2-DES-AGENT-001-execution-engine.md | Execution phases provide the event capture points for observability. |
| related-to | L2-DES-LLM-003 | 1 | specs/L2/llm/L2-DES-LLM-003-model-usage-observability.md | Defines the model-specific usage and streaming trace data used by this architecture. |
| related-to | L2-DES-TOOL-001 | 1 | specs/L2/tool/L2-DES-TOOL-001-built-in-tool-system.md | Tool lifecycle events and result summaries feed user-facing diagnostics. |
| specified-by | L3-BEH-APP-002 | 1 | specs/L3/app/L3-BEH-APP-002-observability-runtime.md | Defines structured log records, active diagnostic projection, trace records, redaction, retention, and telemetry runtime behavior. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-23 | Assistant | Initial | Initial cross-system observability architecture. |

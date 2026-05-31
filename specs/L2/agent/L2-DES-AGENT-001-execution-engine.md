---
artifact_id: L2-DES-AGENT-001
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-22
---

# L2-DES-AGENT-001 — Agent Execution Engine

## Purpose

Define the server-side execution engine that carries an accepted `turn/submit` request through context assembly, model invocation, tool orchestration, durable recording, and terminal turn status.

## Background / Context

Existing L2 designs define important boundaries around the execution engine:

- `L2-DES-APP-003` defines the client/server protocol envelope, request/response behavior, and live event delivery.
- `L2-DES-CONV-001` defines the durable session JSONL data model and replay records.
- `L2-DES-MODEL-001` defines model-provider binding and `ResolvedModelProfile` construction.

Those designs do not define what actually happens inside the server after `turn/submit` is accepted and before `turn_completed`, `turn_failed`, or `turn_interrupted` is emitted. This document fills that gap.

## Source Requirements

- `L1-REQ-AGENT-001` requires a complete task execution workflow from user request to final outcome.
- `L1-REQ-CONV-002` requires observable and durable turn lifecycle behavior.
- `L1-REQ-CONV-003` requires explicit active-turn `steer` and `queue` handling.
- `L1-REQ-CONTEXT-001` requires useful model context management.
- `L1-REQ-CONTEXT-003` requires context compression near model limits.
- `L1-REQ-INPUT-001` requires attachments and multimodal input as first-class task context.
- `L1-REQ-LLM-001` requires token-efficient context construction.
- `L1-REQ-LLM-002` requires controlled model-requested tool use.
- `L1-REQ-LLM-003` requires model usage observability.
- `L1-REQ-LLM-004` requires persona and communication style handling.
- `L1-REQ-MODEL-001` requires model configuration and capability metadata.
- `L1-REQ-TOOL-002` requires baseline built-in tools for coding-agent workflows.
- `L1-REQ-TOOL-001` requires tool safety and redaction.
- `L1-REQ-APP-003` requires permission modes, sandboxing, user approval for out-of-boundary actions, and visible approval outcomes.
- `L1-REQ-GOAL-001` requires bounded autonomous Ralph Loop continuation around a durable objective.
- `L1-REQ-APP-002` requires persistence and recovery behavior.
- `L1-REQ-APP-011` requires actionable error recovery.
- `L2-DES-APP-003` defines protocol requests and notifications around the engine.
- `L2-DES-CONV-001` defines durable turn, item, context, and workspace change records.
- `L2-DES-MODEL-001` defines model-provider resolution used for invocation.
- `L2-DES-TOOL-001` defines the built-in tool registry, lifecycle, and plan tool.

## Design Requirement

The server must own turn execution. Clients submit intent and observe canonical events, but the server is responsible for the execution state machine, context assembly, model calls, tool dispatch, persistence, and terminal outcome.

The execution engine should be deterministic enough that durable records can explain what happened after replay, even though provider streams, external tools, and wall-clock timing are runtime effects.

## Execution Boundary

The execution engine starts after a client request has been accepted as an executable turn.

Input boundary:

- A session identifier or new-session request.
- Accepted user content parts and mentions.
- Current session metadata.
- Effective configuration and model selection.
- Active context state.
- Optional mode, permission, and reasoning overrides allowed by current policy.

Output boundary:

- Durable turn and item records.
- Provider usage records.
- Tool call and tool result records.
- Workspace change-set records where files changed.
- Context snapshot or compaction records where context changed.
- Server-client events for subscribed clients.
- Exactly one terminal turn outcome: completed, failed, or interrupted.

The execution engine does not define the WebSocket transport, the exact JSONL wire format, provider-specific HTTP payloads, or individual tool schemas. Those are defined by adjacent L2/L3 designs.

## Runtime Concepts

Conceptual `TurnExecution` fields:

- `turn_id`
- `session_id`
- `submitted_by_client_id`
- `submission_id`
- `status`: admitted, running, waiting, completed, failed, or interrupted.
- `phase`: admission, context_assembly, model_invocation, tool_dispatch, waiting_for_user, recording, finalization, or terminal.
- `user_item_id`
- `resolved_model_profile`
- `context_snapshot_id`
- `active_invocation_id`
- `active_tool_call_ids`
- `pending_approval_ids`
- `pending_question_ids`
- `workspace_change_set_id`
- `usage_accumulator`
- `interrupt_token`
- `created_at`
- `updated_at`

Public client status should remain simpler than internal phase. For example, `context_assembly`, `model_invocation`, and `tool_dispatch` may all appear as `running`; approval and question waits appear as `waiting`; final outcomes appear as `completed`, `failed`, or `interrupted`.

## Turn Admission

The server should admit a submitted user message according to current session activity:

- If the session has no active turn, the server creates a new `turn_started` record and starts execution.
- If a turn is active and the client submits `steer`, the server records a steer item associated with the active turn.
- If a turn is active and the client submits `queue`, the server records a queue item to execute after the active turn reaches a terminal state.
- If the client submits ordinary input while a turn is active without selecting an allowed mode, the server rejects or reclassifies according to `L1-REQ-CONV-003` and the protocol design.
- If an idempotent retry repeats a previously accepted client-generated message id, the server returns the original canonical ids instead of creating duplicate execution.

Turn admission must persist the accepted input before model invocation begins.

## Execution Flow

The normal execution flow is:

1. Persist accepted user input and `turn_started`.
2. Load or materialize the current session projection from durable state.
3. Resolve the active model binding into a `ResolvedModelProfile`.
4. Assemble model context from instructions, metadata, active context references, user content, mentions, attachments, and tool availability.
5. If context pressure requires compaction, perform or schedule context compression before the primary model request.
6. Start a model invocation using the resolved provider method.
7. Normalize provider stream events into internal runtime events.
8. Persist logical assistant, reasoning, tool-call, usage, and error records at durable granularity.
9. Broadcast coalesced server-client events for live display.
10. Validate model-requested tool calls.
11. Run approval, permission, and safety checks before risky tool execution.
12. Execute approved tool calls through the tool supervisor.
13. Record tool results, workspace changes, output redaction state, and safety notices.
14. Feed tool results back into the model context when the provider interaction continues.
15. Repeat model/tool cycles until the model produces a terminal assistant response or execution stops.
16. Persist terminal status and final usage/context state.
17. Start the next queued item if one is pending and policy permits.

## Context Assembly

Context assembly creates the model-visible request for one invocation. It should be explicit and auditable without treating all assembled content as transcript turns.

Inputs may include:

- Base instructions, active mode instructions, persona, and permission posture from session metadata.
- The active context object from `L2-DES-CONV-001`.
- Visible transcript items selected for the current context window.
- Summaries produced by context compaction.
- User content parts and mentions from the current turn.
- Attachment and multimodal content references.
- Tool schemas and tool availability.
- Internal persistent memory selected by core policy where supported.
- Current model capability and token-budget constraints.

Context assembly should produce a context snapshot reference that can explain which durable records and metadata influenced the invocation. Provider-specific serialization, such as system, developer, user, assistant, or tool messages, is a request-building concern and does not convert metadata instructions into transcript turns.

## Model Invocation

The execution engine should treat provider calls as resumable runtime work around durable logical records:

- Each model call receives an `invocation_id`.
- Provider request metadata should identify the `ResolvedModelProfile`, context snapshot, tool schema set, reasoning effort, and request options.
- Provider streaming deltas should be normalized into provider/core events first.
- The engine should coalesce provider deltas into durable item append records and live client updates according to `L2-DES-CONV-001` and `L2-DES-APP-003`.
- Usage received during or after the invocation should update turn and session usage records.
- Provider errors should become structured turn errors with enough recovery context for user-visible reporting.

The engine should not expose provider-native event streams directly as the client protocol.

## Tool Dispatch

Tool dispatch is owned by the execution engine through a tool supervisor.

For each model-requested tool call, the engine should:

- Capture the model-provided command description for shell or command execution tools.
- Parse and validate structured tool arguments.
- Resolve the tool definition and capability policy.
- Apply permission, safety, approval, and redaction rules.
- Emit visible waiting state if user approval is required.
- Execute allowed tool calls with bounded output capture.
- Support explicit parallel tool groups where enabled.
- Record started, updated, completed, failed, denied, or canceled tool states.
- Return structured tool results with natural-language status summaries to the model when execution continues.

The plan tool is a normal server-owned tool from the execution engine's perspective, but its result updates visible plan state rather than external files or command output.

Structured mutating tools such as `write` and `apply_patch` should report file changes into the core-owned per-turn workspace change set. Shell commands and background processes should report process state through the tool/process supervisor, with file-change attribution only when reliable checkpointing or attribution exists.

## Progress Visibility

The engine should make execution state visible through protocol events, not through client-side inference.

Client-visible progress may include:

- `turn_started`
- `turn_status_changed`
- `item_started`
- `item_content_update`
- `item_completed`
- `tool_call_started`
- `tool_call_updated`
- `tool_call_completed`
- `plan_updated`
- `turn_diff_updated`
- `usage_updated`
- `context_updated`
- `goal_updated`
- `goal_continuation_started`
- `error_reported`
- terminal turn status

Durable records should be written before or atomically with corresponding canonical events where practical, so reconnecting clients can recover state after interruption or crash.

## Goal Integration

Goal-driven continuation turns use the same execution engine as user-submitted turns. The goal system may create hidden continuation input when the session is idle and the active goal is eligible, but once admitted, the turn follows normal context assembly, model invocation, tool dispatch, persistence, and terminal status rules.

Rules:

- The engine should expose usage, tool, and terminal-turn signals needed by the goal system for incremental budget accounting.
- Goal hidden context should be supplied by context assembly as metadata-derived model-visible context, not as a normal transcript item.
- The narrow model-facing goal update tool should be dispatched through the normal tool supervisor.
- If an active goal becomes paused, canceled, blocked, complete, or budget-limited during execution, the current turn may finish or interrupt according to runtime policy, but future autonomous continuation must stop.
- Plan Mode turns must not be launched as autonomous goal continuations.

## Failure Handling

The engine should classify failures by phase:

- Admission failure.
- Context assembly failure.
- Model resolution failure.
- Provider invocation failure.
- Tool validation failure.
- Tool execution failure.
- Approval or question timeout.
- Persistence failure.
- Interruption.

Recoverable failures should preserve completed records and expose a practical next action. Terminal failures should produce a `turn_failed` record rather than silently abandoning the turn.

## Completion Semantics

A turn is complete only after:

- The final assistant-visible response item, if any, has been recorded.
- Required tool results have been recorded.
- Usage totals have been updated where known.
- Workspace change-set state has been finalized where file changes occurred.
- Context state has been updated or left unchanged explicitly.
- The terminal `turn_completed` record has been persisted.
- Subscribed clients have enough events to render the terminal state.

The final user-facing response should summarize the outcome, changed files where relevant, verification performed, and unresolved work.

## Invariants

- At most one owner executes a given active turn.
- Accepted user input is durable before model invocation begins.
- A turn reaches exactly one terminal state.
- Clients observe server-confirmed state; they do not own execution state.
- Durable replay must recover completed, failed, and interrupted turn history.
- Tool calls cannot bypass validation, approval, or safety policy.
- File mutations are attributed to the turn when performed by structured tools or reliable checkpoints.
- Context assembly should avoid duplicating stable prefixes unnecessarily.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-AGENT-001 | 1 | specs/L1/L1-REQ-AGENT-001-execution-workflow.md | Defines the server-side execution engine that carries user intent to terminal task outcome. |
| related-to | L1-REQ-AGENT-002 | 1 | specs/L1/L1-REQ-AGENT-002-interrupt-resume.md | Interrupt and resume act on execution engine runtime state. |
| related-to | L1-REQ-AGENT-003 | 1 | specs/L1/L1-REQ-AGENT-003-task-planning.md | Visible plans are progress state layered on top of execution phases. |
| related-to | L1-REQ-AGENT-004 | 1 | specs/L1/L1-REQ-AGENT-004-subagents.md | Subagents depend on bounded delegated execution units. |
| related-to | L1-REQ-CONV-002 | 1 | specs/L1/L1-REQ-CONV-002-turn-lifecycle.md | Execution produces turn lifecycle states. |
| related-to | L1-REQ-CONV-003 | 1 | specs/L1/L1-REQ-CONV-003-active-turn-message-handling.md | Turn admission handles steer and queue behavior. |
| related-to | L1-REQ-CONTEXT-001 | 1 | specs/L1/L1-REQ-CONTEXT-001-management.md | Execution assembles active context for model invocation. |
| related-to | L1-REQ-LLM-002 | 1 | specs/L1/L1-REQ-LLM-002-tools.md | Execution validates and dispatches model-requested tools. |
| related-to | L1-REQ-TOOL-001 | 1 | specs/L1/L1-REQ-TOOL-001-safety.md | Tool dispatch applies safety and approval rules. |
| related-to | L1-REQ-APP-003 | 1 | specs/L1/L1-REQ-APP-003-safety.md | Execution applies permission, sandbox, and approval checks before risky tool execution. |
| related-to | L1-REQ-TOOL-002 | 1 | specs/L1/L1-REQ-TOOL-002-tools.md | Execution dispatches built-in tools through the tool supervisor. |
| related-to | L1-REQ-GOAL-001 | 1 | specs/L1/L1-REQ-GOAL-001-ralph-loop.md | Goal-driven continuation turns execute through the normal engine and provide budget accounting signals. |
| related-to | L2-DES-APP-003 | 2 | specs/L2/app/L2-DES-APP-003-client-server-protocol.md | Protocol requests and events expose execution state to clients. |
| related-to | L2-DES-CONV-001 | 1 | specs/L2/conv/L2-DES-CONV-001-session-jsonl-data-model.md | Durable records persist execution state. |
| related-to | L2-DES-MODEL-001 | 2 | specs/L2/model/L2-DES-MODEL-001-model-provider-binding.md | Model resolution provides runtime invocation profiles. |
| related-to | L2-DES-TOOL-001 | 1 | specs/L2/tool/L2-DES-TOOL-001-built-in-tool-system.md | Defines tool registry, lifecycle, and plan tool behavior used by dispatch. |
| related-to | L2-DES-GOAL-001 | 1 | specs/L2/goal/L2-DES-GOAL-001-ralph-loop-goals.md | Defines autonomous goal continuation and model-facing goal update behavior layered on the engine. |
| specified-by | L3-BEH-CORE-002 | 3 | specs/L3/core/L3-BEH-CORE-002-turn-execution-engine.md | L3 defines the core turn execution state machine and decision boundaries. |
| specified-by | L3-BEH-SERVER-001 | 4 | specs/L3/server/L3-BEH-SERVER-001-server-runtime-transport.md | L3 defines server orchestration around core turn execution. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-22 | Assistant | Initial | Initial server-side agent execution engine design. |
| 1 | 2026-05-22 | Human | Refinement | Linked execution tool dispatch to the built-in tool system and plan tool. |
| 1 | 2026-05-23 | Human | Refinement | Added goal-driven continuation integration and budget-accounting signal requirements. |
| 1 | 2026-05-25 | Assistant | Refinement | Linked execution permission and approval checks to `L1-REQ-APP-003` application safety. |

---
artifact_id: L2-DES-TOOL-002
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-26
---

# L2-DES-TOOL-002 — Parallel Tool Orchestration

## Purpose

Refine explicit parallel tool orchestration into a concrete server-side design for `multi_tool_use`.

## Background / Context

The model may request a group of independent tool calls through an explicit orchestration tool named `multi_tool_use`. In this product, that request has user-visible semantics: the child calls are intended to run concurrently.

This design adopts the useful separation between model-facing parallel capability and runtime execution, but it does not adopt a group-level reader/writer scheduling model for `multi_tool_use`. The current L1 requirement is stricter: `multi_tool_use` means explicit parallel execution, and the runtime must not serialize, reorder, reject, or downgrade the group solely because of additional runtime parallel-safety classification.

Safety is still mandatory. Every child call follows the same validation, permission, approval, sandbox, availability, redaction, output, observability, and durable-recording behavior it would follow if invoked directly.

## Source Requirements

- `L1-REQ-TOOL-004` requires explicit parallel orchestration where `multi_tool_use` schedules listed tool calls for parallel execution without bypassing ordinary controls.
- `L1-REQ-TOOL-001` requires tool safety, approval, redaction, and bounded output.
- `L1-REQ-TOOL-002` requires baseline built-in tools and controlled tool lifecycle.
- `L1-REQ-APP-003` requires permission modes, sandboxing, and explicit approval for actions outside the current permission boundary.
- `L1-REQ-AGENT-001` requires visible execution workflow from user request to outcome.
- `L1-REQ-CONV-002` requires observable and durable turn lifecycle behavior.
- `L2-DES-TOOL-001` defines the built-in tool registry, lifecycle, and common tool policies.
- `L2-DES-AGENT-001` defines execution engine tool dispatch.
- `L2-DES-APP-003` defines client-visible tool events.
- `L2-DES-CONV-001` defines durable session records.
- `L2-DES-TUI-004` defines live tool rendering in the TUI.

## Design Requirement

`multi_tool_use` is a parent orchestration tool. It accepts an ordered list of child tool calls and schedules all admitted child calls concurrently.

```text
model emits multi_tool_use
        ↓
server validates parent envelope
        ↓
server resolves child tool definitions
        ↓
child lifecycle tasks are created for every listed child
        ↓
each child independently runs normal validation, permission, approval, sandbox, and safety gates
        ↓
admitted children start concurrently
        ↓
child progress and terminal states stream to clients independently
        ↓
parent result aggregates all child terminal results for model continuation
```

The orchestrator is not a safety bypass and not a dependency scheduler. It is a concurrency boundary around ordinary tool invocations.

## Parent Tool Input

The exact provider wire format is L3, but the logical parent input should include:

| Field | Purpose |
|---|---|
| `parallel_group_id` | Stable id for the group, generated or accepted by the server. |
| `calls` | Ordered list of child tool invocations. |
| `description` | Optional concise group-level intent where supported by the model/tool schema. |

Each child call should include:

| Field | Purpose |
|---|---|
| `child_index` | Position in the parent `calls` list. |
| `tool_name` | Target tool. |
| `tool_call_id` | Stable child call id. |
| `arguments` | Tool-specific arguments. |
| `description` | Required only when the target child is a shell or command execution tool that normally requires command intent. |

The parent envelope may be rejected before execution when the group itself is malformed, exceeds configured group limits, contains unknown child records, or cannot be parsed into child tool calls.

## Child Lifecycle

Each child call runs through the same lifecycle as a direct tool call:

1. Resolve tool definition.
2. Validate child arguments against the target tool schema.
3. Evaluate mode, availability, configuration, permission, sandbox, and safety policy.
4. Request approval if the direct child call would require approval.
5. Execute the target tool if admitted.
6. Stream progress where supported.
7. Bound and redact output.
8. Persist child call and child result records.
9. Emit child terminal event.

If a child is blocked before execution, it still receives a child terminal result such as `invalid_input`, `blocked_by_mode`, `needs_approval`, `denied`, `blocked_by_sandbox`, `needs_configuration`, or `unavailable`.

## Concurrency Semantics

The parent orchestrator must schedule every listed child call as a concurrent child task after the parent envelope is accepted.

Rules:

- The orchestrator must not serialize child calls because a child tool lacks a separate `parallel_safe` marker.
- The orchestrator must not reorder child calls to optimize runtime execution.
- The orchestrator must not reject the group solely because it contains mutating tools, command tools, or mixed tool categories.
- A child tool may still be blocked, denied, or failed by the same policy that would apply to a direct invocation.
- A child tool handler may use its own internal resource locking, file locking, process isolation, or sandbox mechanism. That is handler behavior, not parent-level `multi_tool_use` downgrading.
- Child result aggregation preserves the original child order even if child completion order differs.

Concurrent scheduling means children are made runnable independently. It does not guarantee identical start timestamps, identical runtime, or bypass of approval waits.

## Approval Behavior

Approval is per child call.

```text
multi_tool_use
  child 0: read file             starts immediately
  child 1: run shell command     waits for approval
  child 2: grep content          starts immediately
```

Rules:

- A child that requires approval enters a waiting state without blocking unrelated siblings.
- If approval is granted, that child proceeds through normal execution.
- If approval is denied, that child becomes terminal with a denied result.
- A parent group may remain active while waiting for one or more child approvals.
- A parent group does not complete until every child reaches a terminal state.
- Approval of one child does not approve siblings unless the server issued an explicit grouped approval prompt that names those siblings.

## Result Aggregation

Clients receive live per-child events as work happens. The model receives the parent tool result only after all child calls reach terminal state.

The parent result should contain:

- `parallel_group_id`
- `status`
- `children`, ordered by `child_index`
- Each child's `tool_call_id`
- Each child's `tool_name`
- Each child's terminal state
- Each child's result summary or blocked/failed summary
- References to large output or redacted output
- Group-level timing and count summary

Group status should be computed as:

| Status | Meaning |
|---|---|
| `completed` | Every child completed successfully. |
| `partial_failure` | At least one child succeeded and at least one child failed, was denied, was blocked, or was canceled. |
| `failed` | No child succeeded and at least one child failed or was blocked. |
| `interrupted` | The group was interrupted before all children reached non-interrupted terminal states. |
| `canceled` | The group was canceled before meaningful child execution began. |

Sibling success must remain visible even when the group status is `partial_failure` or `failed`.

## Client Events

The server should expose both parent-level and child-level state.

Representative event flow:

```text
tool_call_started(parent multi_tool_use, parallel_group_id)
tool_call_started(child 0, parent_tool_call_id, parallel_group_id, child_index=0)
tool_call_started(child 1, parent_tool_call_id, parallel_group_id, child_index=1)
tool_call_updated(child 1 waiting_for_approval)
tool_call_completed(child 0 completed)
tool_call_completed(child 1 denied)
tool_call_completed(parent multi_tool_use partial_failure)
```

Event payloads should include:

- `parent_tool_call_id` for children.
- `parallel_group_id` for parent and children.
- `child_index` for children.
- `group_child_count` where useful for rendering.
- Per-child `status`, `approval_state`, `safety_state`, `redaction_state`, and `result_summary`.

The TUI may render the parent as a visible parallel group, but it must also render each child result clearly enough for audit.

## Durable Records

The durable session model should preserve:

- Parent `multi_tool_use` call record.
- Child tool call records.
- Parent-child relationships.
- Child order.
- Per-child terminal states and summaries.
- Parent terminal state and aggregate summary.
- Approval request and resolution records for children that require approval.
- Output references for child results where output is large.

Replay should reconstruct both the group and individual child results.

## Model Continuation

The execution engine should not send a partially resolved parent result back to the model while siblings remain active or waiting for approval.

For the next model invocation, the engine should include all child outputs in the parent result in stable child order. This keeps model-visible tool context deterministic even when runtime completion order differs.

```text
runtime completion order:
  child 2, child 0, child 1

model-visible result order:
  child 0, child 1, child 2
```

## Limits

The implementation should define conservative limits in L3:

- Maximum child calls per group.
- Maximum nested orchestration depth. Initial design should reject nested `multi_tool_use` unless explicitly enabled later.
- Per-child timeout.
- Group timeout.
- Per-child output limit.
- Aggregate output limit.

Limit failures should be explicit and should not hide child results already produced.

## Interruption And Cancellation

Turn interruption cancels the parent group and sends cancellation to all non-terminal child tasks.

Rules:

- Completed child results are retained.
- Waiting approval prompts are resolved as interrupted or canceled according to approval-state policy.
- Running child tools receive the same cancellation behavior they would receive outside `multi_tool_use`.
- The parent result becomes `interrupted` when interruption prevents all children from reaching ordinary terminal states.
- Cancellation must not wait indefinitely for a child that cannot stop promptly; bounded cleanup belongs to L3.

## Non-Adopted Runtime Scheduling Model

A prior design option used a session-wide reader/writer scheduling lock and per-tool concurrency-safety declarations. That model is not used as the semantics for `multi_tool_use` because current L1 requirements explicitly reject runtime-selected serialization or downgrade for this tool.

The program may still use internal locks inside specific tool handlers, sandboxes, filesystem operations, or process supervisors. Those locks protect tool correctness. They must not become a parent-level policy that changes `multi_tool_use` from explicit parallel orchestration into runtime-selected serial execution.

If a future requirement introduces provider-native batched tool calls that are not explicit `multi_tool_use`, those calls may receive their own scheduler design. That future scheduler must not change the semantics of `multi_tool_use` without updating `L1-REQ-TOOL-004`.

## Testing Strategy

Required test coverage:

- Parent schema validation rejects malformed child lists before execution.
- Two allowed test tools in one group start concurrently, verified with a barrier or timestamp instrumentation.
- Child results are emitted as each child completes, not only after all siblings complete.
- Parent result waits until all children are terminal before model continuation.
- Result aggregation preserves child order even when completion order differs.
- One child validation failure does not hide successful sibling results.
- One child approval wait does not block unrelated siblings from executing.
- Approval denial produces a child denied result and a parent `partial_failure` or `failed` result as appropriate.
- Shell command children follow ordinary shell validation and approval behavior while still being scheduled in parallel.
- Turn interruption cancels unfinished children and preserves completed siblings.
- Durable replay reconstructs parent group, child order, and per-child terminal states.

Regression coverage should specifically prevent late UI flush behavior where a parallel group emits no child activity until every child finishes. Server-client child events should be emitted independently as child state changes.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-TOOL-004 | 1 | specs/L1/L1-REQ-TOOL-004-parallel-tool-orchestration.md | Defines explicit `multi_tool_use` concurrency semantics, child lifecycle, aggregation, visibility, cancellation, and test strategy. |
| related-to | L1-REQ-TOOL-001 | 1 | specs/L1/L1-REQ-TOOL-001-safety.md | Child calls preserve normal safety, approval, redaction, and output controls. |
| related-to | L1-REQ-TOOL-002 | 1 | specs/L1/L1-REQ-TOOL-002-tools.md | `multi_tool_use` is a built-in orchestration tool. |
| related-to | L1-REQ-APP-003 | 1 | specs/L1/L1-REQ-APP-003-safety.md | Permission, sandbox, and approval checks apply per child call. |
| related-to | L1-REQ-AGENT-001 | 1 | specs/L1/L1-REQ-AGENT-001-execution-workflow.md | Parallel groups execute inside the normal turn execution engine. |
| related-to | L1-REQ-CONV-002 | 1 | specs/L1/L1-REQ-CONV-002-turn-lifecycle.md | Parent and child tool states must be observable and durable within a turn. |
| related-to | L2-DES-TOOL-001 | 1 | specs/L2/tool/L2-DES-TOOL-001-built-in-tool-system.md | Defines common tool lifecycle and policies used by each child call. |
| related-to | L2-DES-AGENT-001 | 1 | specs/L2/agent/L2-DES-AGENT-001-execution-engine.md | The execution engine dispatches and aggregates the parallel group. |
| related-to | L2-DES-APP-003 | 2 | specs/L2/app/L2-DES-APP-003-client-server-protocol.md | Client events expose parent and child tool state. |
| related-to | L2-DES-CONV-001 | 1 | specs/L2/conv/L2-DES-CONV-001-session-jsonl-data-model.md | Durable records preserve parent-child relationships and replay state. |
| related-to | L2-DES-TUI-004 | 1 | specs/L2/tui/L2-DES-TUI-004-streaming-transcript-and-state.md | TUI rendering must show child progress and partial failures. |
| specified-by | L3-BEH-TOOLS-003 | 1 | specs/L3/tools/L3-BEH-TOOLS-003-parallel-orchestration.md | L3 defines parent envelope validation, concurrent child scheduling, result aggregation, client events, and interruption. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-26 | Assistant | Initial | Initial explicit `multi_tool_use` orchestration design adapted to the current L1 requirement. |

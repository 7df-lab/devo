---
artifact_id: L2-DES-AGENT-003
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-06-07
---

# L2-DES-AGENT-003 — Subagent Architecture

## Purpose

Define the architecture for spawning, coordinating, and integrating subagents — independent, bounded execution contexts created as child sessions within a parent session, performing delegated work while the parent monitors their output.

## Scope

This document covers:
- Agent tree model and generated hierarchical naming
- Default child role metadata and parent config inheritance
- Spawn lifecycle (slot reservation, config inheritance, fork modes, initial message delivery)
- Parent-to-child input delivery through child mailboxes
- Child assistant-output buffering and parent polling
- Agent status lifecycle
- Parent-only subagent tool surface (spawn, send message, wait, list, close)
- Depth and concurrency limits
- Persistence of spawn-tree edges for session resumption
- Safety, permission, and approval boundaries for subagents
- Orchestration prompt instructions and subagent-mode tool visibility

This document does **not** cover:
- Session forking implementation details (see L2-DES-CONV-001)
- The execution engine that dispatches subagent turns (see L2-DES-AGENT-001)
- Built-in tool registration mechanics (see L2-DES-TOOL-001)

## Design Decisions

### DD-1: Subagents are child sessions within a shared session tree

A subagent is a lightweight child execution context — a child session — that belongs to the same root session as its parent. It inherits the parent's session-level state (workspace, permissions profile, shell environment) while maintaining an independent conversation transcript. This avoids the heavyweight cost of fully separate root sessions while preserving clear boundaries.

**Decision**: Subagents are spawned as child sessions within the parent session tree. Each child session has its own conversation history, config snapshot, and state. The root session agent (`/root`) is the top of the tree.

### DD-2: Generated hierarchical agent paths provide a stable, navigable identity model

Flat UUIDs are hard for both humans and models to reason about. A tree-structured path model (`/root/researcher/worker`) mirrors the spawn hierarchy, making ownership and relationships immediately visible.

**Decision**: Every agent is assigned a canonical `AgentPath` — a slash-separated hierarchical path rooted at `/root`. The parent does not provide the child name. At spawn time, the runtime generates a unique adjective-noun nickname under the parent and joins it to the parent path. Paths remain stable for the agent's lifetime.

### DD-3: Child sessions inherit parent configuration in the current baseline

The current public spawn tool intentionally keeps configuration simple. A child session inherits the parent's effective model, provider, workspace, permissions, shell environment, and safety posture. The runtime records a role label for metadata, but the model-facing spawn API does not accept role, model, or reasoning overrides in this baseline.

**Decision**: Spawned children use the generated identity plus `agent_role = "default"` metadata and inherit parent configuration. Role-specific configuration can be added later as a new design revision when the public API and safety rules are defined.

### DD-4: Parent-to-child messages are delivered as child user input

Parent agents need to send additional input to child agents without treating the child as a peer chat participant. Child agents do not need to send messages to their parent; the parent observes child output separately.

**Decision**: Each child session has an internal mailbox for parent-to-child text. `send_message` writes to that mailbox and the runtime consumes mailbox entries as normal child user turns. If the child is idle, the message starts a turn immediately. If the child is active, the message is queued for the next child turn. Child-to-parent mailbox routing is not supported.

### DD-5: Parent task waiting is terminal-aware

The parent must be able to monitor child progress and completion without receiving child-authored mailbox messages.

**Decision**: Each spawned child session is also an agent task, identified by a `TaskId` derived from the child session id. Child assistant deltas continue streaming to clients, while the parent output buffer accumulates them without waking the parent model. `await_task` returns only when the child turn reaches a terminal state or the absolute timeout expires. The legacy `wait_agent` tool remains dispatchable as a compatibility adapter but is omitted from new model tool schemas.

### DD-6: Subagents inherit permission and safety boundaries, never bypass them

A spawned subagent must not be a mechanism to escape sandboxing, approval policy, or workspace boundaries. The parent's safety posture must propagate to children.

**Decision**: At spawn time, the child inherits the parent's active permission profile, approval policy, shell environment policy, exec policy, and cwd. Role overrides cannot relax these safety constraints. Subagents are subject to the same `authorize_tool_request` flow as the parent (see L2-DES-SAFETY-002).

## Architecture

### Agent Tree Model

The agent tree organizes subagents into a spawn hierarchy rooted at the root session's parent agent.

```
/root                        ← root session agent (the user's conversation partner)
├── /root/researcher         ← subagent researching subsystem A
│   └── /root/researcher/worker  ← sub-subagent for a subtask
└── /root/implementer        ← subagent implementing subsystem B
```

Each agent in the tree is backed by a child session (a `SessionRecord` with `parent_session_id` set). The parent-child relationship is tracked as a spawn edge in the agent graph store.

#### Agent Identifiers

Each agent has three identification dimensions:

| Identifier | Type | Stability | Purpose |
|------------|------|-----------|---------|
| `session_id` | `SessionId` (UUID) | Stable for lifetime | Internal routing, persistence |
| `agent_path` | `AgentPath` | Stable for lifetime | Human/model-facing identity, inter-agent addressing |
| `agent_nickname` | String | Stable for lifetime | Generated friendly display name (e.g. `brave-apple`) |

#### Agent Metadata

Every tracked agent carries metadata stored in the agent registry:

```
AgentMetadata {
    session_id: Option<SessionId>,
    agent_path: Option<AgentPath>,
    agent_nickname: Option<String>,
    agent_role: Option<String>,
    last_task_message: Option<String>,
}
```

These fields correspond to the existing `SessionRecord` columns: `agent_nickname`, `agent_role`, `agent_path`, and `parent_session_id`.

#### Nickname Pools

Agent nicknames are generated from a fixed ASCII adjective-noun pool. The registry tracks used names under each parent to avoid duplicates. When the pool is exhausted, spawn fails with deterministic invalid input rather than reusing a name.

### Agent Roles

The current baseline records `agent_role = "default"` for each child. It does not expose role selection to the model, and it does not support model or reasoning overrides in `spawn_agent`.

### Spawn Lifecycle

#### Step 1: Model Invocation

The model calls the `spawn_agent` tool with:
- `message`: The initial task description for the new agent
- `fork_turns` (optional): `"none"` (no history) or `"all"` (full history)

#### Step 2: Slot Reservation

The `AgentRegistry` checks concurrent agent limits (`agent_max_sub_agents`). If at capacity, the spawn is rejected with an `AgentLimitReached` error. Otherwise, a `SpawnReservation` is created, reserving a slot.

#### Step 3: Path and Nickname Assignment

The runtime generates a unique adjective-noun nickname under the parent registry and computes the child's `AgentPath` by joining the parent's path with that nickname. If the generated-name pool is exhausted for the parent, spawn is rejected.

#### Step 4: Config Construction

The child session's config is built from the parent's effective config, with runtime turn-state settings applied. The child inherits the parent's model, provider, permissions, shell environment, and cwd.

#### Step 5: Child Session Creation

A new child session is created with:
- `SessionSource::SubAgent { parent_session_id, depth, agent_path, agent_nickname, agent_role }`
- The child's config snapshot
- Inherited shell snapshot and exec policy (from the parent)
- Optionally, forked conversation history (if `fork_turns` is not `"none"`)

The child session's `SessionRecord` stores `parent_session_id`, `agent_path`, `agent_nickname`, and `agent_role` for durable identity.

#### Step 6: Message Delivery

The initial task message is submitted as the child session's first user turn. During model request assembly, request-only subagent reminders are inserted before this task input and are not persisted into the child transcript.

#### Step 7: Spawn-Edge Persistence

The parent-child spawn relationship is persisted to the agent graph store as an `Open` spawn edge. This edge tracks:
- `parent_session_id`
- `child_session_id`
- `status`: `Open` (agent is alive or may be resumed) or `Closed` (agent was explicitly closed)

#### Step 8: Output Buffer Initialization

The parent output buffer accumulates child assistant text and terminal status. The parent waits with `await_task`; UI clients continue receiving live deltas independently.

#### Slot Reservation Lifecycle

```
┌──────────────────┐
│  reserve_spawn   │  Increments active count, creates SpawnReservation
│  _slot()         │
└──────┬───────────┘
       │
       ▼
┌──────────────────┐
│  SpawnReservation │  Holds reserved path and nickname
│  (in-flight)      │
└──────┬───────────┘
       │
       ▼
┌──────────────────┐
│  commit()        │  Registers agent metadata in tree, consumes reservation
│  (on success)     │
└──────────────────┘
       │
       │ (or on failure / drop)
       ▼
┌──────────────────┐
│  release_spawned │  Removes agent from tree, decrements count
│  _agent()        │
│  or drop()       │
└──────────────────┘
```

#### Fork Modes

Subagents may inherit conversation history from the parent through fork modes:

| Mode | Behavior |
|------|----------|
| `none` (default) | No history inherited. Child starts with a clean conversation containing only the initial task message. |
| `all` | Full conversation history up to the spawn point is forked. Assistant reasoning items and intermediate tool calls are filtered out; only user/developer/system messages and final assistant answers are retained. |

Forked history is deduplicated via reference-based storage rather than deep-copied (see L2-DES-CONV-001). When forking full history, the child inherits the parent's model context to maintain consistency.

### Inter-Agent Communication

#### Child Input Mailbox

Each child agent session has an internal mailbox for receiving parent-authored input.

```
Mailbox {
    next_sequence: u64,
    pending: VecDeque<AgentMailboxMessage>,
}
```

Key properties:
- Monotonic sequence numbers: each message gets an incrementing sequence number.
- Queue buffering: parent messages are drained into normal child user turns.
- Directionality: only parent-to-child delivery is supported.

#### Message Structure

```rust
struct AgentMailboxMessage {
    from_session_id: SessionId,
    to_session_id: SessionId,
    from_agent_path: String,
    to_agent_path: String,
    content: String,
    sequence: u64,
}
```

#### Delivery Behavior

`send_message` places the message in the target child's mailbox. The runtime drains mailbox entries as child user input:
- If the child is idle, the drained message starts a child turn immediately.
- If the child is active, the drained message is queued for the next child turn.
- If the caller is a child targeting `parent`, `root`, or the parent session id, the request is rejected.

#### Message Flow

```
Sender Agent                    Mailbox                      Recipient Agent
     │                             │                              │
     │  send_message(target, msg)  │                              │
     │────────────────────────────►│                              │
     │                             │  (queue)                     │
     │                             │                              │
     │                             │  drain as user input         │
     │                             │─────────────────────────────►│
     │                             │  starts turn or queues turn  │
```

#### Output Buffer Polling

The parent does not receive child-authored mailbox messages. Instead, each parent session has an output buffer:

```rust
enum AgentOutputEventKind {
    AssistantMessage,  // full accumulated assistant text for one child turn
    Status,              // terminal turn status (completed, failed, ...)
}

struct ParentAgentOutputEvent {
    sequence: u64,
    agent_path: String,
    agent_nickname: String,
    kind: AgentOutputEventKind,
    text: Option<String>,
    status: Option<String>,
}
```

`await_task` waits for the selected task's current child turn to become terminal. Streaming text does not satisfy the wait predicate. If the absolute deadline expires, it returns current task metadata without partial assistant output. If the task is already terminal, it returns immediately with the accumulated final assistant output.

Default and maximum wait timeouts are server constants (`default_wait_timeout_secs = 5`, `max_wait_timeout_secs = 120`). Callers pass `timeout_secs`.

### Public Task Status Lifecycle

```
Running ──► WaitingApproval ──► Running
   │                                │
   ├──────────────► Completed ◄─────┤
   ├──────────────► Failed
   └──────────────► Canceled
```

| Status | Meaning |
|--------|---------|
| `WaitingApproval` | The active child turn is waiting for an approval decision |
| `Running` | The task is spawning, queued internally, or actively processing |
| `Completed` | The current child turn finished successfully |
| `Failed` | The task encountered a terminal execution failure |
| `Canceled` | The task was interrupted, canceled, or explicitly closed |

Terminal statuses append status events to the parent output buffer.

### Subagent Tool Surface

The subagent tool surface consists of the following built-in tools. All are gated behind a `multi_agent` feature flag.

#### `spawn_agent`

Creates a new subagent (child session) and sends an initial task message.

| Parameter | Required | Type | Description |
|-----------|----------|------|-------------|
| `message` | Yes | string | Initial task description |
| `fork_turns` | No | string | `"all"` (default stable-history fork excluding the active parent turn) or `"none"` (clean child context) |

**Output**: Task id, generated agent path, generated nickname, and public task state. The legacy protocol response may additionally include the child session id and internal agent status.

**Errors**:
- `AgentLimitReached`: Concurrent agent limit exceeded
- Generated name pool exhausted
- Invalid fork_turns value

#### `send_message`

Sends parent-authored text to an existing child agent as child user input.

| Parameter | Required | Type | Description |
|-----------|----------|------|-------------|
| `target` | Yes | string | Target agent path (absolute or relative) |
| `message` | Yes | string | Text message content |

**Output**: Delivery acknowledgment and the reactivated task id.

**Errors**: Target not found, empty message, or caller attempts child-to-parent delivery.

#### `await_task`

Waits for one child task to reach a terminal state.

| Parameter | Required | Type | Description |
|-----------|----------|------|-------------|
| `task_id` | Yes | string | Task id returned by `spawn_agent` or `send_message` |
| `timeout_secs` | No | integer | Absolute wait deadline in seconds (default 5, max 120) |

**Output**: Tagged `terminal` or `timed_out` result. Terminal results include task metadata and final assistant output; timed-out results omit partial output.

**Behavior**: Streaming output remains visible to clients but never wakes the parent model. An already-terminal task returns immediately.

#### `list_tasks`

Lists child tasks, optionally filtered by agent path prefix.

| Parameter | Required | Type | Description |
|-----------|----------|------|-------------|
| `path_prefix` | No | string | Filter agents by path prefix (absolute or relative) |

**Output**: Array of task records containing `task_id`, task kind, one of the five public states, and agent-session metadata (`session_id`, `parent_session_id`, path, nickname, role, and last task message).

The root agent is always included when no prefix or a matching prefix is specified.

#### `cancel_task`

Cancels a direct child task and performs the complete agent-close lifecycle.

| Parameter | Required | Type | Description |
|-----------|----------|------|-------------|
| `task_id` | Yes | string | Task id returned by `spawn_agent` or `send_message` |

**Output**: Final task metadata with state `canceled`.

**Errors**: Target not found.

**Behavior**:
1. Marks the target child as close-requested.
2. Interrupts active target work if needed.
3. Records one terminal `closed` status event for parent polling.

`wait_agent`, `list_agents`, and `close_agent` remain runtime compatibility adapters for older transcripts and clients. They are not included in new model tool schemas.

### Depth and Concurrency Limits

Three configurable limits prevent runaway parallel spawning:

| Limit | Config Key | Scope | Behavior |
|-------|-----------|-------|----------|
| Max concurrent subagents | `agent_max_sub_agents` | Per root session tree | Hard cap on total live agent sessions. Spawn is rejected when at capacity. |
| Max spawn depth | `agent_max_depth` | Per root session tree | Maximum depth from root in the agent tree (root = 0). Spawn is rejected when exceeded. At the depth limit, the child's config has multi-agent features disabled to prevent further nesting. |
| Max concurrent per session | `max_concurrent_sub_agents_per_session` | Per session | Separate cap for multi-agent mode specifically. |

### Persistence and Resumption

#### Spawn-Edge Persistence

Parent-child spawn relationships are persisted as edges in the agent graph store:

```
(parent_session_id, child_session_id, status: Open | Closed)
```

This enables:
- Reconstructing the agent tree when resuming a session
- Finding child sessions that need to be re-attached on resume
- Distinguishing open agents (may still be active) from closed agents (completed and cleaned up)

#### Session Resumption

When a session is resumed:

1. The root session is loaded.
2. Open spawn edges are traversed to discover child sessions.
3. Child sessions are resumed from their persisted rollout transcripts.
4. The `AgentControl` is re-established with the reconstructed registry.
5. Completion watchers are re-attached for any child that was still running when the session was suspended.

The agent registry is rebuilt in-memory from the persisted edges. Resume is recursive: a child's descendants are also discovered and resumed.

#### Spawn-Edge States

| State | Meaning |
|-------|---------|
| `Open` | Agent was spawned and may still be active or resumable |
| `Closed` | Agent was explicitly closed via `cancel_task` or the legacy `close_agent` adapter |

Closing an edge does not delete the child's transcript — closed agents remain in the session history for audit and review.

### Safety, Permission, and Approval Boundaries

Subagents are subject to all normal safety constraints. They do not create a permission bypass.

#### Permission Inheritance

At spawn time, the child inherits:
- Active permission profile (filesystem policy, network policy)
- Approval policy setting
- Shell environment policy
- Execution policy (exec-policy rules)
- Working directory

These are copied from the parent's runtime turn state, not from the original config, ensuring the child sees the parent's currently-effective safety posture.

#### Approval for Subagent Actions

Subagents route their tool calls through the same `authorize_tool_request` path as the parent. This means:
- A subagent whose actions exceed the inherited permission profile will trigger approval prompts.
- The approval reviewer setting (`user` or `auto_review`) applies to subagent actions.
- Approved scopes (session, path, host) are shared within the session tree, not per-agent — a parent's cached approval benefits children and vice versa.

Subagents inherit the parent's session-level approval cache at spawn time, so previously-granted session-scoped approvals remain effective.

#### Depth-Dependent Safeguards

At or beyond `agent_max_depth`, the child's config is locked down:
- `multi_agent` features are disabled, preventing further spawning.
- This prevents unbounded recursive agent trees.

### Context Injection

#### Environment Context

When the agent's environment context is assembled, live child subagents are listed:

```
<environment_context>
Sub-agents:
  /root/brave-apple (brave-apple) — Investigating authentication module
  /root/calm-fox (calm-fox) — Implementing database migration
</environment_context>
```

This gives the parent agent awareness of which children exist and what they're doing.

#### Orchestration Instructions

When multi-agent features are enabled, a dedicated set of instructions is injected into the system prompt. Key orchestration rules include:

- **Prefer multiple sub-agents to parallelize work.** When a task decomposes naturally into independent subtasks, spawn them in parallel rather than sequentially.
- **If sub-agents are running, wait for them before yielding**, unless the user asks an explicit question. If the user asks a question, answer it first, then continue coordinating.
- **When you delegate work to a sub-agent, your role becomes coordination.** Do not perform the actual work while sub-agents are working. Trust their results without redundant verification.
- **Assign clear ownership.** When multiple workers are spawned to modify code, explicitly assign files or modules to each to avoid merge conflicts.
- **Reuse existing sub-agents for related follow-up questions** rather than spawning new ones.
- **Use `send_message`** to send additional user input to an existing child agent.
- **Use `await_task`** to wait for terminal child output, with an appropriate absolute timeout.
- **Use `list_tasks`** for nonblocking status and complete agent-session metadata.
- **Use `cancel_task`** to stop work and close the child agent resources.

These instructions adapt to the active tool surface. Parent sessions can see the agent coordination tools and their schema descriptions. Subagent sessions cannot see or load agent coordination tools, even when the parent used `fork_turns = "all"`.

#### Subagent Mode Reminder

Subagent `ModelRequest.system` contains only the inherited base instructions; deferred-tool reminders and subagent-mode reminders are not appended to the system prompt in subagent mode. Each subagent model request receives request-only reminder user messages after any prefix/environment and inherited stable parent history, but before the current child task input. For `fork_turns = "all"`, this yields stable parent history followed by request-only reminders and then the child task. For `fork_turns = "none"`, this yields prefix/environment, request-only reminders, and then the child task. The reminder states that the model is running as a subagent, must not call agent coordination tools such as `spawn_agent`, and should report results through normal assistant output. These reminders are not persisted into the child transcript, preserving context-prefix stability for full-history forks.

#### Tool Visibility

In subagent mode, `spawn_agent`, `send_message`, `await_task`, `list_tasks`, `cancel_task`, the legacy adapters, and their aliases are hidden from model tool schemas, deferred tool reminders, and `ToolSearch` selection. Runtime dispatch still rejects those calls as defense-in-depth if a model attempts one from inherited context or hallucination.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---:|---|---|---|
| refines | L1-REQ-AGENT-004 | 1 | specs/L1/L1-REQ-AGENT-004-subagents.md | Defines subagent spawn, status inspection, result integration, and forked context. |
| refines | L1-REQ-CONV-004 | 1 | specs/L1/L1-REQ-CONV-004-session-forking.md | Subagent spawn uses session forking for context inheritance. |
| refines | L1-REQ-TOOL-002 | 1 | specs/L1/L1-REQ-TOOL-002-tools.md | Subagent coordination tools defined as built-in delegation tools. |
| related-to | L2-DES-AGENT-001 | 1 | specs/L2/agent/L2-DES-AGENT-001-execution-engine.md | Execution engine dispatches subagent turns. |
| related-to | L2-DES-AGENT-002 | 1 | specs/L2/agent/L2-DES-AGENT-002-interrupt-resume-control.md | Subagents may be interrupted and resumed. |
| related-to | L2-DES-TOOL-001 | 1 | specs/L2/tool/L2-DES-TOOL-001-built-in-tool-system.md | Subagent tools are registered as built-in delegation tools. |
| related-to | L2-DES-SAFETY-001 | 1 | specs/L2/safety/L2-DES-SAFETY-001-permission-system.md | Subagents inherit permission profiles. |
| related-to | L2-DES-SAFETY-002 | 1 | specs/L2/safety/L2-DES-SAFETY-002-approval-mechanism.md | Subagents route through the same approval flow. |
| related-to | L2-DES-CONV-001 | 1 | specs/L2/conv/L2-DES-CONV-001-session-jsonl-data-model.md | Session data model includes fork references and subagent metadata. |
| related-to | L2-DES-APP-003 | 2 | specs/L2/app/L2-DES-APP-003-client-server-protocol.md | Protocol events expose subagent spawn, status, and completion. |
| specified-by | L3-BEH-SERVER-003 | 2 | specs/L3/server/L3-BEH-SERVER-003-subagent-coordination.md | L3 defines spawn lifecycle, agent tree registry, mailbox, completion watching, and session resumption. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-25 | Assistant | Initial | Initial subagent architecture design. |

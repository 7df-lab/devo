---
artifact_id: L3-BEH-CORE-002
revision: 3
status: Draft
active_baseline: no
---

# L3-BEH-CORE-002 — Turn Execution Engine

## Purpose

Define the core-side turn execution contracts that carry an admitted turn through context preparation, provider-event reduction, tool dispatch policy, durable record production, and terminal completion. Server owns runtime orchestration and provider transport; core owns the decisions and state transitions that make the turn replayable.

## Source Design

L2-DES-AGENT-001, L3-DES-ARCH-001

## 1. Ownership Boundary

The execution engine is split across crates:

- **Server owns** active turn slots, task spawning, cancellation token routing, provider adapter invocation, client event sequencing, and WebSocket broadcast.
- **Provider owns** provider-specific HTTP/API request serialization, stream parsing, raw error classification, and provider event normalization.
- **Core owns** turn admission, durable record schemas, context assembly/compaction/normalization, provider-neutral invocation planning, provider event reduction, tool authorization, tool handler execution policy, usage accumulation, and terminal turn decisions.

Core must not open provider HTTP streams directly. Provider-native events must be normalized before core consumes them, and clients must receive program-level events rather than provider-native streams.

## 2. Core Entry Points

The exact module names are implementation choices, but the core crate must expose equivalent behavior.

```rust
pub async fn admit_turn(
    store: &dyn SessionStore,
    session_id: SessionId,
    input: TurnInput,
    admission: TurnAdmissionOptions,
) -> Result<AdmittedTurn, TurnAdmissionError>;

pub async fn prepare_model_invocation(
    store: &dyn SessionStore,
    session: &SessionProjection,
    turn: &TurnProjection,
    registry: &dyn ToolRegistry,
    cancel_token: &CancellationToken,
) -> Result<ModelInvocationPlan, TurnEngineError>;

pub async fn consume_provider_event(
    state: &mut TurnRuntimeState,
    event: ProviderEvent,
) -> Result<ProviderEventReduction, TurnEngineError>;

pub async fn finish_model_invocation(
    state: &mut TurnRuntimeState,
    completion: ModelInvocationCompletion,
) -> Result<ModelInvocationOutcome, TurnEngineError>;

pub async fn execute_tool(
    tool_name: &str,
    input: serde_json::Value,
    ctx: ToolContext,
    progress: Option<ToolProgressSender>,
) -> Result<ToolOutput, ToolError>;
```

### Core Data Shapes

```rust
pub struct AdmittedTurn {
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub turn_kind: TurnKind,
    pub admitted_input_records: Vec<DurableRecord>,
    pub initial_client_events: Vec<ServerClientEvent>,
}

pub struct ModelInvocationPlan {
    pub invocation_id: InvocationId,
    pub turn_id: TurnId,
    pub resolved_model: ResolvedModelProfile,
    pub context_snapshot: AssembledContext,
    pub provider_input: ProviderInvocationInput,
    pub tool_definitions: Vec<ToolDefinition>,
    pub retry_policy: InvocationRetryPolicy,
    pub pre_invocation_records: Vec<DurableRecord>,
    pub pre_invocation_events: Vec<ServerClientEvent>,
}

pub struct ProviderEventReduction {
    pub durable_records: Vec<DurableRecord>,
    pub client_events: Vec<ServerClientEvent>,
    pub newly_completed_tool_calls: Vec<ToolCallItem>,
    pub usage_delta: Option<TurnUsageDelta>,
    pub terminal_signal: Option<ModelTerminalSignal>,
}

pub enum ModelInvocationOutcome {
    TerminalResponse {
        response_item_id: ItemId,
        usage_delta: TurnUsageDelta,
    },
    ToolCallsRequired {
        tool_calls: Vec<ToolCallItem>,
        continuation_context: AssembledContext,
        usage_delta: TurnUsageDelta,
    },
    Failed {
        failure: TurnFailure,
        partial_records: Vec<DurableRecord>,
    },
    Interrupted {
        partial_records: Vec<DurableRecord>,
        cleanup_status: CleanupStatus,
    },
}
```

`ProviderInvocationInput` is provider-neutral. The provider crate serializes it into provider-specific request bodies. This avoids a core-to-provider dependency while still letting core define the exact context, tools, model profile, reasoning settings, hidden goal context, and metadata that must be invoked.

## 3. Execution Phase State Machine

```rust
pub enum ExecutionPhase {
    Admission,
    ContextAssembly,
    Compaction,
    ProviderInvocation,
    ProviderEventReduction,
    ToolDispatch,
    WaitingForUser,
    Finalization,
    Terminal,
}
```

### Transition Table

```text
Admission -> ContextAssembly              accepted input records are durable
Admission -> Terminal(Failed)             admission validation or persistence failure

ContextAssembly -> Compaction             context exceeds compaction threshold
ContextAssembly -> ProviderInvocation     context and provider-neutral plan ready
ContextAssembly -> Terminal(Failed)       model/config/context resolution failure

Compaction -> ProviderInvocation          compaction completed or safely skipped
Compaction -> Terminal(Failed)            context cannot fit after allowed recovery

ProviderInvocation -> ProviderEventReduction  normalized provider event received
ProviderInvocation -> Terminal(Failed)        provider failed before any reducible event
ProviderInvocation -> Terminal(Interrupted)   invocation canceled before completion

ProviderEventReduction -> ProviderInvocation  continue consuming stream
ProviderEventReduction -> ToolDispatch        completed tool calls available
ProviderEventReduction -> Finalization        terminal assistant response complete
ProviderEventReduction -> Terminal(Failed)    malformed normalized event or replay error
ProviderEventReduction -> Terminal(Interrupted) cancellation after partial content

ToolDispatch -> WaitingForUser            approval or question required
ToolDispatch -> ContextAssembly           tool results appended; next model cycle needed
ToolDispatch -> Finalization              turn can complete without another model call
ToolDispatch -> Terminal(Failed)          critical tool failure
ToolDispatch -> Terminal(Interrupted)     cancellation during tool execution

WaitingForUser -> ToolDispatch            user answered pending prompt
WaitingForUser -> Terminal(Interrupted)   user canceled, prompt timed out, or session stopped

Finalization -> Terminal(Completed)       terminal records and usage are durable
Finalization -> Terminal(Failed)          terminal persistence failure
```

### Illegal Transitions

| Transition | Reason |
|---|---|
| `Terminal -> any phase` | Terminal states are final. Resume creates a new linked turn. |
| `ToolDispatch -> ProviderInvocation` | Tool results must be folded into context assembly first. |
| `WaitingForUser -> ProviderInvocation` | The pending prompt must resolve through tool dispatch. |
| `ProviderInvocation -> ToolDispatch` without provider-event reduction | Provider-native data cannot become tool calls directly. |
| `Admission -> ProviderInvocation` | Accepted input must be durable and context must be assembled first. |

## 4. Server-Orchestrated Turn Loop

Server owns the async loop, but each decision point delegates to core or provider.

```rust
async fn execute_turn_loop(
    runtime: &ServerRuntime,
    session_id: SessionId,
    input: TurnInput,
    cancel_token: CancellationToken,
) -> TurnOutcome {
    let admitted = core::admit_turn(runtime.store.as_ref(), session_id, input, options).await?;
    runtime.append_and_broadcast(&admitted.admitted_input_records, &admitted.initial_client_events).await?;

    let mut state = TurnRuntimeState::new(admitted.turn_id);

    loop {
        let session = runtime.store.load_projection(session_id).await?;
        let turn = session.turn(admitted.turn_id)?;

        let plan = core::prepare_model_invocation(
            runtime.store.as_ref(),
            &session,
            &turn,
            runtime.tool_registry.as_ref(),
            &cancel_token,
        ).await?;
        runtime.append_and_broadcast(&plan.pre_invocation_records, &plan.pre_invocation_events).await?;

        let provider_stream = runtime
            .provider_router
            .stream(plan.resolved_model.clone(), plan.provider_input.clone(), cancel_token.clone())
            .await?;

        for event in provider_stream {
            let reduction = core::consume_provider_event(&mut state, event?).await?;
            runtime.append_and_broadcast(&reduction.durable_records, &reduction.client_events).await?;
            if reduction.terminal_signal.is_some() {
                break;
            }
        }

        match core::finish_model_invocation(&mut state, ModelInvocationCompletion::StreamEnded).await? {
            ModelInvocationOutcome::TerminalResponse { .. } => {
                return runtime.finalize_turn(&mut state, TurnStatus::Completed).await;
            }
            ModelInvocationOutcome::ToolCallsRequired { tool_calls, .. } => {
                let tool_results = runtime.dispatch_tools_via_core(&mut state, tool_calls, &cancel_token).await?;
                core::append_tool_results(&mut state, tool_results)?;
                continue;
            }
            ModelInvocationOutcome::Failed { failure, partial_records } => {
                runtime.append_records(&partial_records).await?;
                return runtime.finalize_failed_turn(&mut state, failure).await;
            }
            ModelInvocationOutcome::Interrupted { partial_records, cleanup_status } => {
                runtime.append_records(&partial_records).await?;
                return runtime.finalize_interrupted_turn(&mut state, cleanup_status).await;
            }
        }
    }
}
```

If provider invocation returns a transport error before stream events are available, server converts it into `ModelInvocationCompletion::ProviderFailed` and calls core finalization so durable failure records are still produced by core schemas.

## 5. Tool Dispatch Contract

Tool dispatch uses the normal tool lifecycle from `L3-BEH-TOOLS-002` and handler catalog from `L3-BEH-CORE-003`.

Core must:

1. Validate tool name and arguments against `ToolSpec`.
2. Enforce session mode gating, including Plan Mode mutating-tool prohibition.
3. Call `authorize_tool_request()` before any effectful handler runs.
4. Return approval or question waits as explicit waiting states, not as provider errors.
5. Execute admitted handlers with `ToolContext`, `CancellationToken`, and optional `ToolProgressSender`.
6. Convert handler output into durable tool records, bounded model-facing content, safe client display content, and workspace change records when applicable.
7. Append tool results to the continuation context before the next model cycle.

Server must schedule concurrency, cancellation, and progress broadcast, but it must not reinterpret permission decisions or invent tool result records.

## 6. Provider Event Reduction

Core consumes normalized `ProviderEvent` values and emits:

- durable records for item start, content append, item completion/failure, tool-call structure, usage, and invocation completion;
- server-client event intents for live rendering;
- usage deltas for goal accounting and session metadata;
- terminal signals for response completion, tool dispatch, provider failure, or interruption.

Reduction rules:

1. Provider deltas are buffered and coalesced before durable `item_content_appended` records are emitted.
2. Live client updates may be more frequent than durable records, but both must reconstruct the same final item content after completion.
3. Provider-native event names and raw payloads are kept out of normal client events and transcript records.
4. Trace mode may record safe provider timing metadata through observability L3, but trace records are not replay authority.
5. Missing usage is represented as unavailable, not zero.

## 7. Failure Classification

```rust
pub enum TurnFailurePhase {
    Admission,
    ContextAssembly,
    Compaction,
    ProviderRequestBuild,
    ProviderTransport,
    ProviderEventReduction,
    ToolValidation,
    ToolExecution,
    ApprovalTimeout,
    QuestionTimeout,
    Persistence,
    Cancelled,
}

pub struct TurnFailure {
    pub phase: TurnFailurePhase,
    pub error_code: String,
    pub message: String,
    pub recoverable: bool,
    pub retry_strategy: Option<RetryStrategy>,
    pub provider_error_ref: Option<String>,
}
```

Provider-specific diagnostics must be referenced or summarized safely. Plaintext credentials, raw request bodies, and unredacted provider payloads must not be stored in durable session records.

## 8. Async Behavior

| Operation | Timeout | Retries | Cancel Behavior |
|---|---|---|---|
| `prepare_model_invocation()` | 60s default | none | Check token before compaction and before returning plan |
| Provider stream invocation | provider/profile configured | retry only when provider policy permits and no non-idempotent tool result has been consumed | Drop/abort provider stream; flush reduced partial content |
| `consume_provider_event()` | cooperative, per event | none | Flush buffers and return interrupted reduction |
| `execute_tool()` | per-tool timeout | no automatic retry by default | Handler-specific cancellation; preserve partial output |
| Tool dispatch group | per child timeout | no automatic retry by default | Cancel selected child or group; preserve completed sibling results |
| Finalization | 30s default | retry append/fsync according to store policy | If finalization cannot persist, surface `Persistence` failure |

## 9. Invariants

- Accepted user input is durable before provider invocation begins.
- Every accepted turn reaches exactly one terminal turn status.
- Provider HTTP/API transport is not performed by core.
- Provider-native stream events are normalized before core reduction.
- Durable records are append-only and sufficient for replay to reconstruct the same logical session state.
- Tool calls route through validation, permission, approval, and mode gating before execution.
- User-visible client events are projections of core/provider/tool state, not alternate sources of truth.
- A resumed turn is a new linked turn; terminal states are not reopened.

## 10. Required Tests

Implementation must include tests or verifier fixtures for:

1. Admission persists `TurnStarted` and user input records before any provider invocation is attempted.
2. `prepare_model_invocation()` returns a provider-neutral plan and does not call provider HTTP/API code.
3. Provider deltas reduce into ordered durable records and live event intents with identical final content.
4. Provider transport failure before first event produces core-authored durable failure records.
5. Tool-call provider events cannot bypass core tool validation and approval.
6. Plan Mode blocks mutating and command tools during dispatch.
7. Interrupt during provider streaming flushes partial content and emits one interrupted terminal turn.
8. Interrupt during a tool preserves completed sibling tool results and marks incomplete tools accurately.
9. Missing provider usage is replayed as unavailable rather than zero.
10. Replay of the durable records from a completed turn reconstructs the same turn status, items, tool calls, and usage summary.

## Traceability

| L2 Source | Relationship |
|---|---|
| L2-DES-AGENT-001 | specified-by |
| L3-DES-ARCH-001 | specified-by |

## Implementation Placement Guidance

- Existing `query()` naming may be reused only if it is refactored to respect the provider transport boundary above. A function named `query()` must not hide provider HTTP transport inside core.
- Server orchestration may keep a compact loop, but provider invocation, provider event reduction, tool dispatch, and finalization must remain separately testable.
- CancellationToken is from `tokio_util::sync::CancellationToken` or an equivalent cooperative cancellation primitive.

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-27 | Assistant | Initial | Initial turn execution state machine and `query()` entry point. |
| 2 | 2026-05-27 | Assistant | Correction | Clarified stale implementation references and decision boundaries. |
| 3 | 2026-05-27 | Assistant | Correction | Replaced ambiguous `query()`-as-provider-call boundary with server-owned provider transport, core provider-event reduction, concrete entry points, invariants, and tests. |

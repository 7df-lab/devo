//! Turn execution engine — core entry points, data shapes, and state machine.
//!
//! Implements L3-BEH-CORE-002. Core owns admission, context preparation,
//! provider-event reduction, tool dispatch policy, and terminal decisions.
//! Server owns transport, provider invocation, and event broadcast.

use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::context_pipeline::{
    AssembledContext, ContextAssembler, ContextConfig, ContextEntry as PipelineContextEntry,
};
use crate::durable_record::{
    ContentPart, DurableRecord, InvocationId, Mention, ModelBindingId, ProviderId,
};
use crate::session_store::SessionStore;
use devo_protocol::{
    ItemId, ModelCatalog, ReasoningEffort, SessionId, ToolDefinition, TurnId, TurnKind, TurnStatus,
    TurnUsage,
};

// ── Turn Admission ──────────────────────────────────────────────────

/// Validated turn input accepted from the client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnInput {
    pub session_id: SessionId,
    pub turn_kind: TurnKind,
    pub user_items: Vec<UserInputItem>,
    pub submitted_by_client_id: Option<String>,
}

/// A single user-authored input item with content and mentions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserInputItem {
    pub item_id: ItemId,
    pub content_parts: Vec<ContentPart>,
    pub mentions: Vec<Mention>,
}

/// Options that control turn admission behavior.
#[derive(Debug, Clone)]
pub struct TurnAdmissionOptions {
    pub model: Option<String>,
    pub reasoning_effort_selection: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub mode_overrides: Option<TurnModeOverrides>,
}

#[derive(Debug, Clone)]
pub struct TurnModeOverrides {
    pub plan_mode: bool,
}

/// Result of a successful turn admission.
#[derive(Debug, Clone)]
pub struct AdmittedTurn {
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub turn_kind: TurnKind,
    pub admitted_input_records: Vec<DurableRecord>,
    pub initial_client_events: Vec<TurnClientEvent>,
}

/// Error returned when turn admission fails.
#[derive(Debug, Clone, thiserror::Error)]
pub enum TurnAdmissionError {
    #[error("session not found: {0}")]
    SessionNotFound(SessionId),
    #[error("another turn is already active in this session")]
    ActiveTurnConflict,
    #[error("persistence failure: {0}")]
    PersistenceFailure(String),
    #[error("invalid turn kind for current session state")]
    InvalidTurnKind,
}

// ── Model Invocation ────────────────────────────────────────────────

/// Provider-neutral invocation plan produced by core.
#[derive(Debug, Clone)]
pub struct ModelInvocationPlan {
    pub invocation_id: InvocationId,
    pub turn_id: TurnId,
    pub resolved_model: ResolvedModelProfile,
    pub context_snapshot: AssembledContext,
    pub provider_input: ProviderInvocationInput,
    pub tool_definitions: Vec<ToolDefinition>,
    pub retry_policy: InvocationRetryPolicy,
    pub pre_invocation_records: Vec<DurableRecord>,
    pub pre_invocation_events: Vec<TurnClientEvent>,
}

/// Resolved model profile for an invocation (merged from catalog + binding + session).
#[derive(Debug, Clone)]
pub struct ResolvedModelProfile {
    pub canonical_model_slug: String,
    pub provider_id: ProviderId,
    pub model_binding_id: ModelBindingId,
    pub display_name: String,
    pub context_window: u64,
    pub effective_context_window: u64,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub modalities: Vec<String>,
}

/// Assembled context is defined in context_pipeline and re-exported here.

#[derive(Debug, Clone)]
pub enum ContextEntry {
    SystemPrompt(String),
    ToolDefinition(String),
    TranscriptItem { turn_id: TurnId, item_id: ItemId },
    ContextSummary(String),
    InstructionFile { path: String, content: String },
}

/// Provider-neutral input that the provider crate serializes into requests.
#[derive(Debug, Clone)]
pub struct ProviderInvocationInput {
    pub context_entries: Vec<ContextEntry>,
    pub tool_definitions: Vec<ToolDefinition>,
    pub model_profile: ResolvedModelProfile,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub max_output_tokens: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct InvocationRetryPolicy {
    pub max_retries: u32,
    pub retry_on: Vec<String>,
    pub backoff_ms: u64,
}

// ── Provider Event Reduction ────────────────────────────────────────

/// Normalized provider event consumed by core.
#[derive(Debug, Clone)]
pub enum ProviderEvent {
    LlmRequestStarted,
    ReasoningStarted {
        item_id: ItemId,
    },
    ReasoningDelta {
        item_id: ItemId,
        delta: String,
    },
    ReasoningCompleted {
        item_id: ItemId,
    },
    AssistantResponseStarted {
        item_id: ItemId,
    },
    TextDelta {
        item_id: ItemId,
        delta: String,
    },
    ToolCallStarted {
        item_id: ItemId,
        tool_call_id: String,
        tool_name: String,
    },
    ToolCallInputDelta {
        tool_call_id: String,
        delta: String,
    },
    ToolCallCompleted {
        tool_call_id: String,
    },
    UsageDelta {
        usage: TurnUsageDelta,
    },
    LlmRequestCompleted {
        usage: Option<TurnUsageDelta>,
    },
    LlmRequestFailed {
        error: String,
        retryable: bool,
    },
    StreamEnded,
}

/// The result of reducing one provider event.
#[derive(Debug, Clone)]
pub struct ProviderEventReduction {
    pub durable_records: Vec<DurableRecord>,
    pub client_events: Vec<TurnClientEvent>,
    pub newly_completed_tool_calls: Vec<PendingToolCall>,
    pub usage_delta: Option<TurnUsageDelta>,
    pub terminal_signal: Option<ModelTerminalSignal>,
}

/// A completed tool call extracted from provider events.
#[derive(Debug, Clone)]
pub struct PendingToolCall {
    pub tool_call_id: String,
    pub tool_name: String,
    pub input: serde_json::Value,
}

/// Signal that the model stream has reached a decision point.
#[derive(Debug, Clone)]
pub enum ModelTerminalSignal {
    ResponseComplete,
    ToolCallsReady,
    ProviderFailed { error: String, retryable: bool },
    Interrupted,
}

/// Delta usage from one provider event or completion.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TurnUsageDelta {
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_creation_input_tokens: Option<i64>,
    pub cache_read_input_tokens: Option<i64>,
    pub reasoning_output_tokens: Option<i64>,
}

/// Completion signal after provider stream ends.
#[derive(Debug, Clone)]
pub enum ModelInvocationCompletion {
    StreamEnded,
    ProviderFailed { error: String },
    Cancelled,
}

/// Outcome determined by core after finishing a model invocation.
#[derive(Debug, Clone)]
pub enum ModelInvocationOutcome {
    TerminalResponse {
        response_item_id: ItemId,
        usage_delta: TurnUsageDelta,
    },
    ToolCallsRequired {
        tool_calls: Vec<PendingToolCall>,
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

// ── Turn Runtime State ──────────────────────────────────────────────

/// Mutable state tracked during a single turn execution.
#[derive(Debug, Clone)]
pub struct TurnRuntimeState {
    pub turn_id: TurnId,
    pub session_id: SessionId,
    pub phase: TurnExecutionPhase,
    pub pending_items: Vec<PendingItem>,
    pub pending_tool_calls: Vec<PendingToolCall>,
    pub accumulated_usage: TurnUsageDelta,
    pub durable_records_since_last_flush: Vec<DurableRecord>,
    pub client_events_since_last_flush: Vec<TurnClientEvent>,
}

impl TurnRuntimeState {
    pub fn new(turn_id: TurnId, session_id: SessionId) -> Self {
        Self {
            turn_id,
            session_id,
            phase: TurnExecutionPhase::Admission,
            pending_items: Vec::new(),
            pending_tool_calls: Vec::new(),
            accumulated_usage: TurnUsageDelta::default(),
            durable_records_since_last_flush: Vec::new(),
            client_events_since_last_flush: Vec::new(),
        }
    }

    pub fn drain_records(&mut self) -> Vec<DurableRecord> {
        std::mem::take(&mut self.durable_records_since_last_flush)
    }

    pub fn drain_events(&mut self) -> Vec<TurnClientEvent> {
        std::mem::take(&mut self.client_events_since_last_flush)
    }
}

/// A streaming item being built up during provider event reduction.
#[derive(Debug, Clone)]
pub struct PendingItem {
    pub item_id: ItemId,
    pub kind: PendingItemKind,
    pub content_parts: Vec<(u32, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingItemKind {
    Reasoning,
    AssistantText,
    ToolCall,
}

// ── Turn Execution Phase State Machine ──────────────────────────────

/// Fine-grained execution phases of a turn (L3-BEH-CORE-002 §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnExecutionPhase {
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

impl TurnExecutionPhase {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Terminal)
    }

    pub fn can_transition_to(&self, next: TurnExecutionPhase) -> Result<(), &'static str> {
        use TurnExecutionPhase::*;
        match (self, next) {
            // Legal transitions
            (Admission, ContextAssembly) => Ok(()),
            (Admission, Terminal) => Ok(()),
            (ContextAssembly, Compaction) => Ok(()),
            (ContextAssembly, ProviderInvocation) => Ok(()),
            (ContextAssembly, Terminal) => Ok(()),
            (Compaction, ProviderInvocation) => Ok(()),
            (Compaction, Terminal) => Ok(()),
            (ProviderInvocation, ProviderEventReduction) => Ok(()),
            (ProviderInvocation, Terminal) => Ok(()),
            (ProviderEventReduction, ProviderInvocation) => Ok(()),
            (ProviderEventReduction, ToolDispatch) => Ok(()),
            (ProviderEventReduction, Finalization) => Ok(()),
            (ProviderEventReduction, Terminal) => Ok(()),
            (ToolDispatch, WaitingForUser) => Ok(()),
            (ToolDispatch, ContextAssembly) => Ok(()),
            (ToolDispatch, Finalization) => Ok(()),
            (ToolDispatch, Terminal) => Ok(()),
            (WaitingForUser, ToolDispatch) => Ok(()),
            (WaitingForUser, Terminal) => Ok(()),
            (Finalization, Terminal) => Ok(()),

            // Terminal is final
            (Terminal, _) => Err("terminal states are final"),

            // All other transitions are illegal
            _ => Err("illegal transition"),
        }
    }
}

// ── Failure Types ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

#[derive(Debug, Clone)]
pub struct TurnFailure {
    pub phase: TurnFailurePhase,
    pub error_code: String,
    pub message: String,
    pub recoverable: bool,
    pub retry_strategy: Option<RetryStrategy>,
    pub provider_error_ref: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RetryStrategy {
    pub max_retries: u32,
    pub backoff_ms: u64,
    pub backoff_multiplier: f64,
}

// ── Cancellation ─────────────────────────────────────────────────────

/// Cooperative cancellation token for turn execution.
#[derive(Debug, Clone)]
pub struct CancellationToken {
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self {
            cancelled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    pub fn cancel(&self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

// ── Cleanup ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupStatus {
    AllRecordsFlushed,
    PartialFlush,
    FlushFailed,
}

// ── Client Events ───────────────────────────────────────────────────

/// Core-emitted client event intents (server converts to wire events).
#[derive(Debug, Clone)]
pub enum TurnClientEvent {
    TurnStarted {
        turn_id: TurnId,
        sequence: u32,
    },
    ItemStarted {
        item_id: ItemId,
        kind: String,
    },
    ItemContentUpdated {
        item_id: ItemId,
        content: String,
    },
    ItemCompleted {
        item_id: ItemId,
    },
    ToolCallStarted {
        tool_call_id: String,
        tool_name: String,
    },
    ToolCallCompleted {
        tool_call_id: String,
    },
    TurnStatusChanged {
        turn_id: TurnId,
        status: TurnStatus,
    },
    TurnUsageUpdated {
        turn_id: TurnId,
        usage: TurnUsageDelta,
    },
    ApprovalRequested {
        approval_id: String,
    },
    TurnCompleted {
        turn_id: TurnId,
        usage: Option<TurnUsage>,
    },
    TurnFailed {
        turn_id: TurnId,
        error: String,
    },
    TurnInterrupted {
        turn_id: TurnId,
    },
}

// ── Core Entry Points (stubs) ───────────────────────────────────────

/// Admit a turn: validate input, write durable records, return an AdmittedTurn.
///
/// Per L3-BEH-CORE-002 §2: writes `TurnStarted` and user input records before returning.
pub async fn admit_turn(
    store: &dyn SessionStore,
    session_id: SessionId,
    input: TurnInput,
    admission: TurnAdmissionOptions,
) -> Result<AdmittedTurn, TurnAdmissionError> {
    let turn_id = TurnId::new();
    let turn_kind = input.turn_kind;
    let now = Utc::now();

    // Write TurnStarted durable record
    let turn_started = DurableRecord::TurnStarted(crate::durable_record::TurnStartedRecord {
        schema_version: 1,
        session_id,
        turn_id,
        sequence: 0,
        status: TurnStatus::Running,
        kind: turn_kind.clone(),
        resume_of_turn_id: None,
        submitted_by_client_id: input.submitted_by_client_id,
        model: admission.model,
        reasoning_effort_selection: admission.reasoning_effort_selection,
        reasoning_effort: admission.reasoning_effort,
        started_at: now,
    });

    let records = vec![turn_started];
    let _offset = store
        .append(session_id, records[0].clone())
        .await
        .map_err(|e| TurnAdmissionError::PersistenceFailure(e.message))?;

    Ok(AdmittedTurn {
        session_id,
        turn_id,
        turn_kind,
        admitted_input_records: records,
        initial_client_events: vec![TurnClientEvent::TurnStarted {
            turn_id,
            sequence: 0,
        }],
    })
}

/// Prepare a provider-neutral invocation plan.
///
/// Core assembles context, resolves the model profile, collects tool definitions,
/// and produces a plan that server uses to invoke the provider.
#[allow(clippy::too_many_arguments)]
pub async fn prepare_model_invocation(
    _store: &dyn SessionStore,
    session: &SessionProjection,
    turn: &TurnProjection,
    registry: &dyn ToolRegistry,
    model_catalog: &dyn ModelCatalog,
    base_instructions: &str,
    project_instructions: &[String],
    active_skills: &[String],
    memory_context: Option<&str>,
    goal_context: Option<&str>,
    cancel_token: &CancellationToken,
) -> Result<ModelInvocationPlan, TurnEngineError> {
    if cancel_token.is_cancelled() {
        return Err(TurnEngineError::Cancelled);
    }

    let invocation_id = InvocationId::new();

    // Resolve model profile from session metadata using the catalog
    let model_slug = session.metadata.model.as_deref();
    let resolved_model_def = model_catalog
        .resolve_for_turn(model_slug)
        .map_err(|e| TurnEngineError::ModelResolutionFailed(e.to_string()))?;

    let resolved_model = ResolvedModelProfile {
        canonical_model_slug: resolved_model_def.slug.clone(),
        provider_id: ProviderId::new(),
        model_binding_id: ModelBindingId::new(),
        display_name: resolved_model_def.display_name.clone(),
        context_window: resolved_model_def.context_window as u64,
        effective_context_window: resolved_model_def.effective_context_window() as u64,
        reasoning_effort: None,
        modalities: vec!["text".to_string()],
    };

    // Collect tool definitions from registry
    let tool_definitions = registry.list_definitions();

    // Assemble context using the context pipeline
    let assembler = ContextAssembler::new(ContextConfig::default());
    let assembled = assembler.assemble(
        session.session_id,
        turn.turn_id,
        base_instructions,
        &[],  // prior_transcript (empty for now; populated by replay)
        None, // persona
        None, // collaboration_mode
        project_instructions,
        active_skills,
        memory_context,
        goal_context,
        None, // change_signal
        None, // user_input (already in turn)
    );

    // Build provider-neutral invocation input from assembled context
    let context_entries: Vec<ContextEntry> = assembled
        .entries
        .iter()
        .map(|entry| match entry {
            PipelineContextEntry::Instruction { content, .. } => {
                ContextEntry::SystemPrompt(content.clone())
            }
            PipelineContextEntry::TranscriptItem { turn_id, item_id } => {
                ContextEntry::TranscriptItem {
                    turn_id: *turn_id,
                    item_id: *item_id,
                }
            }
            PipelineContextEntry::TranscriptRange { from, to } => {
                ContextEntry::ContextSummary(format!("transcript range {}..{}", from, to))
            }
            PipelineContextEntry::ContextSummary { summary_id } => {
                ContextEntry::ContextSummary(summary_id.clone())
            }
            PipelineContextEntry::Artifact { artifact_id } => ContextEntry::InstructionFile {
                path: artifact_id.clone(),
                content: String::new(),
            },
        })
        .collect();

    let provider_input = ProviderInvocationInput {
        context_entries,
        tool_definitions: tool_definitions.clone(),
        model_profile: resolved_model.clone(),
        reasoning_effort: resolved_model.reasoning_effort,
        max_output_tokens: None,
    };

    Ok(ModelInvocationPlan {
        invocation_id,
        turn_id: turn.turn_id,
        resolved_model,
        context_snapshot: assembled,
        provider_input,
        tool_definitions,
        retry_policy: InvocationRetryPolicy {
            max_retries: 1,
            retry_on: vec!["RateLimitError".into(), "ProviderServerError".into()],
            backoff_ms: 1000,
        },
        pre_invocation_records: vec![],
        pre_invocation_events: vec![],
    })
}

/// Reduce one normalized provider event into durable records and client events.
///
/// Per L3-BEH-CORE-002 §6:
/// - Coalesces text/reasoning deltas before emitting durable ItemContentAppended.
/// - Emits live client events for every delta.
/// - Tracks usage deltas and tool calls.
/// - Produces terminal signals at stream boundaries.
pub async fn consume_provider_event(
    state: &mut TurnRuntimeState,
    event: ProviderEvent,
) -> Result<ProviderEventReduction, TurnEngineError> {
    let mut durable_records = Vec::new();
    let mut client_events = Vec::new();
    let mut newly_completed_tool_calls = Vec::new();
    let mut usage_delta: Option<TurnUsageDelta> = None;
    let mut terminal_signal: Option<ModelTerminalSignal> = None;

    match event {
        ProviderEvent::LlmRequestStarted => {
            client_events.push(TurnClientEvent::TurnStatusChanged {
                turn_id: state.turn_id,
                status: devo_protocol::TurnStatus::Running,
            });
        }

        ProviderEvent::ReasoningStarted { item_id } => {
            state.pending_items.push(PendingItem {
                item_id,
                kind: PendingItemKind::Reasoning,
                content_parts: Vec::new(),
            });
            durable_records.push(DurableRecord::ItemStarted(
                crate::durable_record::ItemStartedRecord {
                    schema_version: 1,
                    session_id: state.session_id,
                    turn_id: state.turn_id,
                    item_id,
                    kind: crate::durable_record::ItemRecordKind::AssistantReasoning,
                    role: crate::durable_record::RecordRole::Assistant,
                    content_parts: vec![],
                    mentions: vec![],
                    visibility: crate::durable_record::ItemVisibility::Visible,
                    created_at: chrono::Utc::now(),
                },
            ));
            client_events.push(TurnClientEvent::ItemStarted {
                item_id,
                kind: "reasoning".into(),
            });
        }

        ProviderEvent::ReasoningDelta { item_id, delta } => {
            client_events.push(TurnClientEvent::ItemContentUpdated {
                item_id,
                content: delta.clone(),
            });
            // Buffer for coalesced durable write
            if let Some(pending) = state
                .pending_items
                .iter_mut()
                .find(|p| p.item_id == item_id)
            {
                pending.content_parts.push((0, delta));
            }
        }

        ProviderEvent::ReasoningCompleted { item_id } => {
            // Flush buffered content as durable record
            if let Some(pending) = state.pending_items.iter().find(|p| p.item_id == item_id) {
                let combined: String = pending
                    .content_parts
                    .iter()
                    .map(|(_, s)| s.as_str())
                    .collect();
                durable_records.push(DurableRecord::ItemContentAppended(
                    crate::durable_record::ItemContentAppendedRecord {
                        schema_version: 1,
                        item_id,
                        content_part_index: 0,
                        offset: 0,
                        content_kind: crate::durable_record::ContentAppendKind::Reasoning,
                        content: combined,
                        byte_count: 0,
                    },
                ));
            }
            durable_records.push(DurableRecord::ItemCompleted(
                crate::durable_record::ItemCompletedRecord {
                    schema_version: 1,
                    item_id,
                    turn_id: state.turn_id,
                    final_status: crate::durable_record::ItemStatus::Completed,
                    content_hash: None,
                    completed_at: chrono::Utc::now(),
                },
            ));
            state.pending_items.retain(|p| p.item_id != item_id);
            client_events.push(TurnClientEvent::ItemCompleted { item_id });
        }

        ProviderEvent::AssistantResponseStarted { item_id } => {
            state.pending_items.push(PendingItem {
                item_id,
                kind: PendingItemKind::AssistantText,
                content_parts: Vec::new(),
            });
            durable_records.push(DurableRecord::ItemStarted(
                crate::durable_record::ItemStartedRecord {
                    schema_version: 1,
                    session_id: state.session_id,
                    turn_id: state.turn_id,
                    item_id,
                    kind: crate::durable_record::ItemRecordKind::AssistantText,
                    role: crate::durable_record::RecordRole::Assistant,
                    content_parts: vec![],
                    mentions: vec![],
                    visibility: crate::durable_record::ItemVisibility::Visible,
                    created_at: chrono::Utc::now(),
                },
            ));
            client_events.push(TurnClientEvent::ItemStarted {
                item_id,
                kind: "text".into(),
            });
        }

        ProviderEvent::TextDelta { item_id, delta } => {
            client_events.push(TurnClientEvent::ItemContentUpdated {
                item_id,
                content: delta.clone(),
            });
            if let Some(pending) = state
                .pending_items
                .iter_mut()
                .find(|p| p.item_id == item_id)
            {
                pending.content_parts.push((0, delta));
            }
        }

        ProviderEvent::ToolCallStarted {
            item_id,
            tool_call_id,
            tool_name,
        } => {
            state.pending_items.push(PendingItem {
                item_id,
                kind: PendingItemKind::ToolCall,
                content_parts: Vec::new(),
            });
            durable_records.push(DurableRecord::ItemStarted(
                crate::durable_record::ItemStartedRecord {
                    schema_version: 1,
                    session_id: state.session_id,
                    turn_id: state.turn_id,
                    item_id,
                    kind: crate::durable_record::ItemRecordKind::ToolCall,
                    role: crate::durable_record::RecordRole::Assistant,
                    content_parts: vec![],
                    mentions: vec![],
                    visibility: crate::durable_record::ItemVisibility::Visible,
                    created_at: chrono::Utc::now(),
                },
            ));
            client_events.push(TurnClientEvent::ToolCallStarted {
                tool_call_id: tool_call_id.clone(),
                tool_name: tool_name.clone(),
            });
            state.pending_tool_calls.push(PendingToolCall {
                tool_call_id,
                tool_name,
                input: serde_json::Value::Null,
            });
        }

        ProviderEvent::ToolCallInputDelta {
            tool_call_id,
            delta,
        } => {
            if let Some(tc) = state
                .pending_tool_calls
                .iter_mut()
                .find(|t| t.tool_call_id == tool_call_id)
            {
                // Accumulate JSON fragments — simplified approach
                if let serde_json::Value::String(ref mut s) = tc.input {
                    s.push_str(&delta);
                } else {
                    tc.input = serde_json::Value::String(delta);
                }
            }
        }

        ProviderEvent::ToolCallCompleted { tool_call_id } => {
            let tcid = tool_call_id.clone();
            // Flush tool call item
            if let Some(pending) = state.pending_items.iter().find(|p| {
                state
                    .pending_tool_calls
                    .iter()
                    .any(|t| t.tool_call_id == tcid)
                    && matches!(p.kind, PendingItemKind::ToolCall)
            }) {
                durable_records.push(DurableRecord::ItemCompleted(
                    crate::durable_record::ItemCompletedRecord {
                        schema_version: 1,
                        item_id: pending.item_id,
                        turn_id: state.turn_id,
                        final_status: crate::durable_record::ItemStatus::Completed,
                        content_hash: None,
                        completed_at: chrono::Utc::now(),
                    },
                ));
            }
            state
                .pending_items
                .retain(|p| !matches!(p.kind, PendingItemKind::ToolCall));
            client_events.push(TurnClientEvent::ToolCallCompleted { tool_call_id: tcid });

            // Move completed tool calls to output
            if let Some(idx) = state
                .pending_tool_calls
                .iter()
                .position(|t| t.tool_call_id == tool_call_id)
            {
                newly_completed_tool_calls.push(state.pending_tool_calls.remove(idx));
            }
        }

        ProviderEvent::UsageDelta { usage } => {
            accumulate_usage(&mut state.accumulated_usage, &usage);
            usage_delta = Some(usage.clone());
            client_events.push(TurnClientEvent::TurnUsageUpdated {
                turn_id: state.turn_id,
                usage,
            });
        }

        ProviderEvent::LlmRequestCompleted { usage } => {
            if let Some(ref u) = usage {
                accumulate_usage(&mut state.accumulated_usage, u);
            }
            usage_delta = usage;
            terminal_signal = Some(ModelTerminalSignal::ResponseComplete);
        }

        ProviderEvent::LlmRequestFailed { error, retryable } => {
            terminal_signal = Some(ModelTerminalSignal::ProviderFailed { error, retryable });
        }

        ProviderEvent::StreamEnded => {
            terminal_signal = Some(ModelTerminalSignal::ResponseComplete);
        }
    }

    Ok(ProviderEventReduction {
        durable_records,
        client_events,
        newly_completed_tool_calls,
        usage_delta,
        terminal_signal,
    })
}

/// Accumulate a usage delta into an accumulator.
fn accumulate_usage(acc: &mut TurnUsageDelta, delta: &TurnUsageDelta) {
    if let Some(v) = delta.input_tokens {
        acc.input_tokens = Some(acc.input_tokens.unwrap_or(0) + v);
    }
    if let Some(v) = delta.output_tokens {
        acc.output_tokens = Some(acc.output_tokens.unwrap_or(0) + v);
    }
    if let Some(v) = delta.cache_creation_input_tokens {
        acc.cache_creation_input_tokens = Some(acc.cache_creation_input_tokens.unwrap_or(0) + v);
    }
    if let Some(v) = delta.cache_read_input_tokens {
        acc.cache_read_input_tokens = Some(acc.cache_read_input_tokens.unwrap_or(0) + v);
    }
    if let Some(v) = delta.reasoning_output_tokens {
        acc.reasoning_output_tokens = Some(acc.reasoning_output_tokens.unwrap_or(0) + v);
    }
}

/// Finish a model invocation and determine the terminal outcome.
///
/// Examines accumulated tool calls and completion status to decide whether
/// the turn is done (TerminalResponse), needs more tool cycles (ToolCallsRequired),
/// or ended with failure/interruption.
pub async fn finish_model_invocation(
    state: &mut TurnRuntimeState,
    completion: ModelInvocationCompletion,
) -> Result<ModelInvocationOutcome, TurnEngineError> {
    match completion {
        ModelInvocationCompletion::StreamEnded => {
            if !state.pending_tool_calls.is_empty() {
                // Model requested tools — extract and return
                let calls: Vec<PendingToolCall> = std::mem::take(&mut state.pending_tool_calls);
                let usage = state.accumulated_usage.clone();
                Ok(ModelInvocationOutcome::ToolCallsRequired {
                    tool_calls: calls,
                    continuation_context: AssembledContext {
                        context_id: format!("cont-{}", uuid::Uuid::new_v4()),
                        session_id: state.session_id,
                        created_for_turn: state.turn_id,
                        entries: vec![],
                        token_estimate: 0,
                        immutable_prefix_hash: String::new(),
                        created_at: chrono::Utc::now(),
                    },
                    usage_delta: usage,
                })
            } else {
                // Clean terminal response
                let response_item_id = devo_protocol::ItemId::new();
                let usage = state.accumulated_usage.clone();
                Ok(ModelInvocationOutcome::TerminalResponse {
                    response_item_id,
                    usage_delta: usage,
                })
            }
        }
        ModelInvocationCompletion::ProviderFailed { error } => Ok(ModelInvocationOutcome::Failed {
            failure: TurnFailure {
                phase: TurnFailurePhase::ProviderTransport,
                error_code: "PROVIDER_FAILED".into(),
                message: error,
                recoverable: true,
                retry_strategy: None,
                provider_error_ref: None,
            },
            partial_records: state.drain_records(),
        }),
        ModelInvocationCompletion::Cancelled => Ok(ModelInvocationOutcome::Interrupted {
            partial_records: state.drain_records(),
            cleanup_status: CleanupStatus::PartialFlush,
        }),
    }
}

// ── Traits needed by core entry points ──────────────────────────────

/// Projected session state for core consumption.
///
/// Uses the protocol's SessionMetadata directly to avoid duplication.
#[derive(Debug, Clone)]
pub struct SessionProjection {
    pub session_id: SessionId,
    pub metadata: devo_protocol::SessionMetadata,
}

/// Projected turn state for core consumption.
#[derive(Debug, Clone)]
pub struct TurnProjection {
    pub turn_id: TurnId,
    pub sequence: u32,
    pub status: TurnStatus,
    pub kind: TurnKind,
    pub items: Vec<ItemProjection>,
}

#[derive(Debug, Clone)]
pub struct ItemProjection {
    pub item_id: ItemId,
    pub kind: String,
    pub content: String,
}

/// Minimal tool registry trait for core's invocation planning.
#[async_trait]
pub trait ToolRegistry: Send + Sync {
    fn list_definitions(&self) -> Vec<ToolDefinition>;
}

// ── Error Type ──────────────────────────────────────────────────────

#[derive(Debug, Clone, thiserror::Error)]
pub enum TurnEngineError {
    #[error("not implemented")]
    NotImplemented,
    #[error("context assembly failed: {0}")]
    ContextAssemblyFailed(String),
    #[error("model resolution failed: {0}")]
    ModelResolutionFailed(String),
    #[error("persistence failure: {0}")]
    PersistenceFailure(String),
    #[error("cancelled")]
    Cancelled,
    #[error("invalid state transition: {0}")]
    InvalidStateTransition(String),
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── TurnExecutionPhase State Machine ──────────────────────────

    #[test]
    fn admission_to_context_assembly_is_legal() {
        assert!(
            TurnExecutionPhase::Admission
                .can_transition_to(TurnExecutionPhase::ContextAssembly)
                .is_ok()
        );
    }

    #[test]
    fn admission_to_provider_invocation_is_illegal() {
        assert!(
            TurnExecutionPhase::Admission
                .can_transition_to(TurnExecutionPhase::ProviderInvocation)
                .is_err()
        );
    }

    #[test]
    fn tool_dispatch_to_context_assembly_is_legal() {
        assert!(
            TurnExecutionPhase::ToolDispatch
                .can_transition_to(TurnExecutionPhase::ContextAssembly)
                .is_ok()
        );
    }

    #[test]
    fn tool_dispatch_to_provider_invocation_is_illegal() {
        assert!(
            TurnExecutionPhase::ToolDispatch
                .can_transition_to(TurnExecutionPhase::ProviderInvocation)
                .is_err()
        );
    }

    #[test]
    fn waiting_for_user_to_provider_invocation_is_illegal() {
        assert!(
            TurnExecutionPhase::WaitingForUser
                .can_transition_to(TurnExecutionPhase::ProviderInvocation)
                .is_err()
        );
    }

    #[test]
    fn terminal_is_terminal() {
        assert!(TurnExecutionPhase::Terminal.is_terminal());
        assert!(!TurnExecutionPhase::Admission.is_terminal());
        assert!(!TurnExecutionPhase::ProviderInvocation.is_terminal());
    }

    #[test]
    fn terminal_cannot_transition() {
        assert!(
            TurnExecutionPhase::Terminal
                .can_transition_to(TurnExecutionPhase::Admission)
                .is_err()
        );
        assert!(
            TurnExecutionPhase::Terminal
                .can_transition_to(TurnExecutionPhase::ContextAssembly)
                .is_err()
        );
    }

    #[test]
    fn all_legal_transitions_are_valid() {
        let legal_pairs = [
            (
                TurnExecutionPhase::Admission,
                TurnExecutionPhase::ContextAssembly,
            ),
            (TurnExecutionPhase::Admission, TurnExecutionPhase::Terminal),
            (
                TurnExecutionPhase::ContextAssembly,
                TurnExecutionPhase::Compaction,
            ),
            (
                TurnExecutionPhase::ContextAssembly,
                TurnExecutionPhase::ProviderInvocation,
            ),
            (
                TurnExecutionPhase::ContextAssembly,
                TurnExecutionPhase::Terminal,
            ),
            (
                TurnExecutionPhase::Compaction,
                TurnExecutionPhase::ProviderInvocation,
            ),
            (TurnExecutionPhase::Compaction, TurnExecutionPhase::Terminal),
            (
                TurnExecutionPhase::ProviderInvocation,
                TurnExecutionPhase::ProviderEventReduction,
            ),
            (
                TurnExecutionPhase::ProviderInvocation,
                TurnExecutionPhase::Terminal,
            ),
            (
                TurnExecutionPhase::ProviderEventReduction,
                TurnExecutionPhase::ProviderInvocation,
            ),
            (
                TurnExecutionPhase::ProviderEventReduction,
                TurnExecutionPhase::ToolDispatch,
            ),
            (
                TurnExecutionPhase::ProviderEventReduction,
                TurnExecutionPhase::Finalization,
            ),
            (
                TurnExecutionPhase::ProviderEventReduction,
                TurnExecutionPhase::Terminal,
            ),
            (
                TurnExecutionPhase::ToolDispatch,
                TurnExecutionPhase::WaitingForUser,
            ),
            (
                TurnExecutionPhase::ToolDispatch,
                TurnExecutionPhase::ContextAssembly,
            ),
            (
                TurnExecutionPhase::ToolDispatch,
                TurnExecutionPhase::Finalization,
            ),
            (
                TurnExecutionPhase::ToolDispatch,
                TurnExecutionPhase::Terminal,
            ),
            (
                TurnExecutionPhase::WaitingForUser,
                TurnExecutionPhase::ToolDispatch,
            ),
            (
                TurnExecutionPhase::WaitingForUser,
                TurnExecutionPhase::Terminal,
            ),
            (
                TurnExecutionPhase::Finalization,
                TurnExecutionPhase::Terminal,
            ),
        ];
        for (from, to) in &legal_pairs {
            assert!(
                from.can_transition_to(*to).is_ok(),
                "transition from {from:?} to {to:?} should be legal"
            );
        }
    }

    #[test]
    fn all_illegal_transitions_are_rejected() {
        let illegal_pairs = [
            (
                TurnExecutionPhase::Admission,
                TurnExecutionPhase::ProviderInvocation,
            ),
            (
                TurnExecutionPhase::ToolDispatch,
                TurnExecutionPhase::ProviderInvocation,
            ),
            (
                TurnExecutionPhase::WaitingForUser,
                TurnExecutionPhase::ProviderInvocation,
            ),
            (
                TurnExecutionPhase::WaitingForUser,
                TurnExecutionPhase::ContextAssembly,
            ),
            (
                TurnExecutionPhase::ProviderInvocation,
                TurnExecutionPhase::ToolDispatch,
            ),
            (
                TurnExecutionPhase::Admission,
                TurnExecutionPhase::Compaction,
            ),
            (
                TurnExecutionPhase::Admission,
                TurnExecutionPhase::ProviderEventReduction,
            ),
            (
                TurnExecutionPhase::Admission,
                TurnExecutionPhase::ToolDispatch,
            ),
            (
                TurnExecutionPhase::Admission,
                TurnExecutionPhase::WaitingForUser,
            ),
            (
                TurnExecutionPhase::Admission,
                TurnExecutionPhase::Finalization,
            ),
        ];
        for (from, to) in &illegal_pairs {
            assert!(
                from.can_transition_to(*to).is_err(),
                "transition from {from:?} to {to:?} should be illegal"
            );
        }
    }

    // ── TurnRuntimeState ──────────────────────────────────────────

    #[test]
    fn turn_runtime_state_starts_in_admission() {
        let state = TurnRuntimeState::new(TurnId::new(), SessionId::new());
        assert_eq!(state.phase, TurnExecutionPhase::Admission);
        assert!(state.pending_items.is_empty());
        assert!(state.accumulated_usage.input_tokens.is_none());
    }

    #[test]
    fn drain_clears_buffers() {
        let mut state = TurnRuntimeState::new(TurnId::new(), SessionId::new());
        state
            .durable_records_since_last_flush
            .push(DurableRecord::TurnStarted(
                crate::durable_record::TurnStartedRecord {
                    schema_version: 1,
                    session_id: SessionId::new(),
                    turn_id: TurnId::new(),
                    sequence: 0,
                    status: TurnStatus::Running,
                    kind: TurnKind::Regular,
                    resume_of_turn_id: None,
                    submitted_by_client_id: None,
                    model: None,
                    reasoning_effort_selection: None,
                    reasoning_effort: None,
                    started_at: Utc::now(),
                },
            ));

        let drained = state.drain_records();
        assert_eq!(drained.len(), 1);
        assert!(state.durable_records_since_last_flush.is_empty());
    }

    // ── TurnExecutionPhase serde ──────────────────────────────────

    #[test]
    fn turn_execution_phase_serde_roundtrip() {
        let phases = [
            TurnExecutionPhase::Admission,
            TurnExecutionPhase::ContextAssembly,
            TurnExecutionPhase::Compaction,
            TurnExecutionPhase::ProviderInvocation,
            TurnExecutionPhase::ProviderEventReduction,
            TurnExecutionPhase::ToolDispatch,
            TurnExecutionPhase::WaitingForUser,
            TurnExecutionPhase::Finalization,
            TurnExecutionPhase::Terminal,
        ];
        for phase in &phases {
            let json = serde_json::to_string(phase).expect("serialize");
            let restored: TurnExecutionPhase = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored, *phase);
        }
    }

    // ── TurnFailure ───────────────────────────────────────────────

    #[test]
    fn turn_failure_phase_serde_roundtrip() {
        let phases = [
            TurnFailurePhase::Admission,
            TurnFailurePhase::ContextAssembly,
            TurnFailurePhase::Compaction,
            TurnFailurePhase::ProviderRequestBuild,
            TurnFailurePhase::ProviderTransport,
            TurnFailurePhase::ProviderEventReduction,
            TurnFailurePhase::ToolValidation,
            TurnFailurePhase::ToolExecution,
            TurnFailurePhase::ApprovalTimeout,
            TurnFailurePhase::QuestionTimeout,
            TurnFailurePhase::Persistence,
            TurnFailurePhase::Cancelled,
        ];
        for phase in &phases {
            let json = serde_json::to_string(phase).expect("serialize");
            let restored: TurnFailurePhase = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored, *phase);
        }
    }

    #[test]
    fn turn_failure_with_retry_strategy() {
        let failure = TurnFailure {
            phase: TurnFailurePhase::ProviderTransport,
            error_code: "RATE_LIMITED".into(),
            message: "Rate limit exceeded".into(),
            recoverable: true,
            retry_strategy: Some(RetryStrategy {
                max_retries: 3,
                backoff_ms: 1000,
                backoff_multiplier: 2.0,
            }),
            provider_error_ref: None,
        };
        assert!(failure.recoverable);
        assert_eq!(failure.retry_strategy.as_ref().unwrap().max_retries, 3);
    }

    #[test]
    fn turn_failure_non_recoverable() {
        let failure = TurnFailure {
            phase: TurnFailurePhase::ToolExecution,
            error_code: "TOOL_CRASHED".into(),
            message: "Tool process exited".into(),
            recoverable: false,
            retry_strategy: None,
            provider_error_ref: None,
        };
        assert!(!failure.recoverable);
        assert!(failure.retry_strategy.is_none());
    }

    // ── ModelInvocationOutcome ────────────────────────────────────

    #[test]
    fn outcome_terminal_response() {
        let item_id = ItemId::new();
        let outcome = ModelInvocationOutcome::TerminalResponse {
            response_item_id: item_id,
            usage_delta: TurnUsageDelta {
                output_tokens: Some(100),
                ..Default::default()
            },
        };
        match outcome {
            ModelInvocationOutcome::TerminalResponse {
                response_item_id,
                usage_delta,
            } => {
                assert_eq!(response_item_id, item_id);
                assert_eq!(usage_delta.output_tokens, Some(100));
            }
            _ => panic!("expected TerminalResponse"),
        }
    }

    #[test]
    fn outcome_tool_calls_required() {
        let calls = vec![PendingToolCall {
            tool_call_id: "call-1".into(),
            tool_name: "read".into(),
            input: serde_json::json!({"path": "src/main.rs"}),
        }];
        let ctx = AssembledContext {
            context_id: "ctx-1".into(),
            session_id: SessionId::new(),
            created_for_turn: TurnId::new(),
            entries: vec![],
            token_estimate: 5000,
            immutable_prefix_hash: "hash".into(),
            created_at: chrono::Utc::now(),
        };
        let outcome = ModelInvocationOutcome::ToolCallsRequired {
            tool_calls: calls,
            continuation_context: ctx,
            usage_delta: TurnUsageDelta::default(),
        };
        match outcome {
            ModelInvocationOutcome::ToolCallsRequired { tool_calls, .. } => {
                assert_eq!(tool_calls.len(), 1);
                assert_eq!(tool_calls[0].tool_name, "read");
            }
            _ => panic!("expected ToolCallsRequired"),
        }
    }

    // ── CleanupStatus serde ───────────────────────────────────────

    #[test]
    fn cleanup_status_serde_roundtrip() {
        for status in &[
            CleanupStatus::AllRecordsFlushed,
            CleanupStatus::PartialFlush,
            CleanupStatus::FlushFailed,
        ] {
            let json = serde_json::to_string(status).expect("serialize");
            let restored: CleanupStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored, *status);
        }
    }

    // ── TurnUsageDelta ────────────────────────────────────────────

    #[test]
    fn usage_delta_default_is_all_none() {
        let delta = TurnUsageDelta::default();
        assert!(delta.input_tokens.is_none());
        assert!(delta.output_tokens.is_none());
    }

    #[test]
    fn usage_delta_serde_roundtrip() {
        let delta = TurnUsageDelta {
            input_tokens: Some(1000),
            output_tokens: Some(500),
            cache_creation_input_tokens: Some(200),
            cache_read_input_tokens: Some(300),
            reasoning_output_tokens: Some(150),
        };
        let json = serde_json::to_string(&delta).expect("serialize");
        let restored: TurnUsageDelta = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.input_tokens, Some(1000));
        assert_eq!(restored.reasoning_output_tokens, Some(150));
    }

    // ── ProviderEvent ─────────────────────────────────────────────

    #[test]
    fn provider_event_variants_are_constructible() {
        let item_id = ItemId::new();
        let events = vec![
            ProviderEvent::LlmRequestStarted,
            ProviderEvent::ReasoningStarted { item_id },
            ProviderEvent::ReasoningDelta {
                item_id,
                delta: "think".into(),
            },
            ProviderEvent::ReasoningCompleted { item_id },
            ProviderEvent::AssistantResponseStarted { item_id },
            ProviderEvent::TextDelta {
                item_id,
                delta: "hello".into(),
            },
            ProviderEvent::ToolCallStarted {
                item_id,
                tool_call_id: "t1".into(),
                tool_name: "read".into(),
            },
            ProviderEvent::ToolCallInputDelta {
                tool_call_id: "t1".into(),
                delta: "{\"path\"".into(),
            },
            ProviderEvent::ToolCallCompleted {
                tool_call_id: "t1".into(),
            },
            ProviderEvent::UsageDelta {
                usage: TurnUsageDelta::default(),
            },
            ProviderEvent::LlmRequestCompleted { usage: None },
            ProviderEvent::LlmRequestFailed {
                error: "timeout".into(),
                retryable: true,
            },
            ProviderEvent::StreamEnded,
        ];
        assert_eq!(events.len(), 13);
    }
}

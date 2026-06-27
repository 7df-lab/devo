use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use devo_core::SessionState;
use devo_protocol::RequestUserInputQuestion;
use devo_protocol::ServerRequestKind;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::session_context::SessionRuntimeContext;

pub(crate) const RESEARCH_FILE_TOOL_NAMES: &[&str] = &["read", "write", "apply_patch"];
const RESEARCH_NO_TOOL_NAMES: &[&str] = &[];
const RESEARCH_CLARIFICATION_TOOL_NAMES: &[&str] = &["request_user_input"];
const RESEARCH_SUPERVISOR_TOOL_NAMES: &[&str] = &[
    "spawn_agent",
    "send_message",
    "wait_agent",
    "list_agents",
    "close_agent",
];
pub(crate) const RESEARCH_WORKER_TOOL_NAMES: &[&str] =
    &["read", "write", "apply_patch", "web_search", "webfetch"];

#[derive(Debug, Clone, Copy)]
enum ResearchStageKind {
    Clarify,
    Brief,
    Supervisor,
    Compress,
    FinalReport,
}

impl ResearchStageKind {
    fn prompt(self) -> String {
        match self {
            ResearchStageKind::Clarify => devo_core::research::prompts::clarify(),
            ResearchStageKind::Brief => devo_core::research::prompts::research_brief(),
            ResearchStageKind::Supervisor => devo_core::research::prompts::supervisor(),
            ResearchStageKind::Compress => devo_core::research::prompts::compress(),
            ResearchStageKind::FinalReport => devo_core::research::prompts::final_report(),
        }
    }

    fn tool_names(self) -> &'static [&'static str] {
        match self {
            ResearchStageKind::Clarify => RESEARCH_CLARIFICATION_TOOL_NAMES,
            ResearchStageKind::Brief | ResearchStageKind::Compress => RESEARCH_NO_TOOL_NAMES,
            ResearchStageKind::Supervisor => RESEARCH_SUPERVISOR_TOOL_NAMES,
            ResearchStageKind::FinalReport => RESEARCH_FILE_TOOL_NAMES,
        }
    }

    fn usage_prefix(self) -> &'static str {
        match self {
            ResearchStageKind::Clarify => "clarify_call",
            ResearchStageKind::Brief => "brief_call",
            ResearchStageKind::Supervisor => "supervisor_call",
            ResearchStageKind::Compress => "supervisor_compress_call",
            ResearchStageKind::FinalReport => "final_report_call",
        }
    }

    fn artifact(self) -> Option<StreamedResearchArtifact> {
        match self {
            ResearchStageKind::Brief => Some(StreamedResearchArtifact {
                artifact_type: ResearchArtifactType::Brief,
                title: "Research Brief".to_string(),
            }),
            ResearchStageKind::Supervisor => Some(StreamedResearchArtifact {
                artifact_type: ResearchArtifactType::Plan,
                title: "Research Plan".to_string(),
            }),
            ResearchStageKind::Compress => Some(StreamedResearchArtifact {
                artifact_type: ResearchArtifactType::CompressedFinding,
                title: "Compressed Finding: Research Evidence".to_string(),
            }),
            ResearchStageKind::Clarify | ResearchStageKind::FinalReport => None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ClarifyDecision {
    need_clarification: bool,
    #[serde(default)]
    question: String,
    #[serde(default)]
    verification: String,
}

#[derive(Debug, Clone)]
struct ResearchRequestContext {
    question: String,
    current_date: String,
    timezone: String,
    cwd: String,
    clarifications: Vec<ResearchClarificationContext>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResearchClarificationContext {
    question: String,
    answer: String,
}

struct ClarificationGateResult {
    artifact_content: String,
    clarifications: Vec<ResearchClarificationContext>,
}

struct SupervisorOutput {
    notes: String,
    worker_count: usize,
}

impl ResearchRequestContext {
    fn new(question: &str, current_date: String, timezone: String, cwd: String) -> Self {
        Self {
            question: question.to_string(),
            current_date,
            timezone,
            cwd,
            clarifications: Vec::new(),
        }
    }

    fn session_messages(&self, additional_context: Vec<String>) -> Vec<devo_core::Message> {
        self.context_texts(additional_context)
            .into_iter()
            .map(devo_core::Message::user)
            .collect()
    }

    fn context_texts(&self, additional_context: Vec<String>) -> Vec<String> {
        let mut messages = vec![
            devo_core::research::prompts::environment_context(
                &self.current_date,
                &self.timezone,
                &self.cwd,
            ),
            self.question.clone(),
        ];
        for clarification in &self.clarifications {
            messages.push(devo_core::research::prompts::clarification_context(
                &clarification.question,
                &clarification.answer,
            ));
        }
        messages.extend(
            additional_context
                .into_iter()
                .filter(|context| !context.trim().is_empty()),
        );
        messages
    }
}

#[derive(Default)]
struct ResearchQueryCapture {
    text: String,
    assistant: StreamedTextItem,
    pending_tools: HashMap<String, PendingResearchToolCall>,
    final_report_write: Option<FinalReportWrite>,
    reasoning: StreamedTextItem,
    usage_invocation_index: usize,
    turn_completed: bool,
}

#[derive(Default)]
struct ClarificationQueryCapture {
    text: String,
    pending_request_user_input_questions: HashMap<String, Vec<(String, String)>>,
    request_user_input_exchanges: Vec<ResearchClarificationContext>,
    clarifications: Vec<ResearchClarificationContext>,
    reasoning: StreamedTextItem,
    usage_invocation_index: usize,
}

#[derive(Default)]
struct SupervisorQueryCapture {
    text: String,
    artifact: StreamedTextItem,
    pending_tools: HashMap<String, PendingResearchToolCall>,
    reasoning: StreamedTextItem,
    usage_invocation_index: usize,
    spawned_worker_count: usize,
}

#[derive(Default)]
struct ResearchArtifactQueryCapture {
    text: String,
    artifact: StreamedTextItem,
    reasoning: StreamedTextItem,
    usage_invocation_index: usize,
    turn_completed: bool,
}

enum ResearchStageCapture<'a> {
    Clarification(&'a mut ClarificationQueryCapture),
    Artifact(&'a mut ResearchArtifactQueryCapture),
    Supervisor(&'a mut SupervisorQueryCapture),
    FinalReport(&'a mut ResearchQueryCapture),
}

struct ResearchQueryEventContext<'a> {
    session_id: SessionId,
    turn_id: TurnId,
    usage_ledger: &'a ResearchUsageLedgerRef,
    context_window: Option<u64>,
}

struct ResearchArtifactEventContext<'a> {
    query: ResearchQueryEventContext<'a>,
    artifact: &'a StreamedResearchArtifact,
}

struct PendingResearchToolCall {
    item_id: ItemId,
    item_seq: u64,
    tool_name: String,
    input: serde_json::Value,
}

#[derive(Debug, Clone)]
struct FinalReportWrite {
    path: String,
    content: String,
}

#[derive(Debug, Default)]
struct StreamedTextItem {
    item_id: Option<ItemId>,
    item_seq: Option<u64>,
    text: String,
}

#[derive(Debug, Clone)]
struct StreamedResearchArtifact {
    artifact_type: ResearchArtifactType,
    title: String,
}

struct ResearchPipelineInput<'a> {
    runtime_context: Arc<SessionRuntimeContext>,
    turn_config: TurnConfig,
    display_input: &'a str,
    question: &'a str,
    cwd: String,
    usage_ledger: ResearchUsageLedgerRef,
}

struct ExecuteResearchTurnInput {
    session_id: SessionId,
    turn: TurnMetadata,
    runtime_context: Arc<SessionRuntimeContext>,
    turn_config: TurnConfig,
    display_input: String,
    question: String,
    cwd: String,
}

struct ResearchModelRuntime<'a> {
    runtime_context: Arc<SessionRuntimeContext>,
    turn_config: &'a TurnConfig,
    usage_ledger: &'a ResearchUsageLedgerRef,
    session_id: SessionId,
    turn_id: TurnId,
}

type ResearchUsageLedgerRef = Arc<Mutex<ResearchUsageLedger>>;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ResearchUsageTotals {
    input_tokens: usize,
    output_tokens: usize,
    total_tokens: usize,
    cache_creation_input_tokens: usize,
    cache_read_input_tokens: usize,
    reasoning_output_tokens: usize,
}

impl ResearchUsageTotals {
    fn from_usage(usage: &devo_protocol::Usage) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            total_tokens: usage.display_total_tokens(),
            cache_creation_input_tokens: usage.cache_creation_input_tokens.unwrap_or(0),
            cache_read_input_tokens: usage.cache_read_input_tokens.unwrap_or(0),
            reasoning_output_tokens: usage.reasoning_output_tokens.unwrap_or(0),
        }
    }

    fn add(&mut self, other: Self) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.total_tokens += other.total_tokens;
        self.cache_creation_input_tokens += other.cache_creation_input_tokens;
        self.cache_read_input_tokens += other.cache_read_input_tokens;
        self.reasoning_output_tokens += other.reasoning_output_tokens;
    }

    fn to_turn_usage(self) -> TurnUsage {
        TurnUsage::from_usage(&devo_protocol::Usage {
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cache_creation_input_tokens: (self.cache_creation_input_tokens > 0)
                .then_some(self.cache_creation_input_tokens),
            cache_read_input_tokens: (self.cache_read_input_tokens > 0)
                .then_some(self.cache_read_input_tokens),
            reasoning_output_tokens: (self.reasoning_output_tokens > 0)
                .then_some(self.reasoning_output_tokens),
            total_tokens: (self.total_tokens > 0).then_some(self.total_tokens),
        })
    }
}

#[derive(Debug)]
struct ResearchUsageLedger {
    base: ResearchUsageTotals,
    by_invocation: HashMap<String, ResearchUsageTotals>,
}

impl ResearchUsageLedger {
    fn new(base: ResearchUsageTotals) -> Self {
        Self {
            base,
            by_invocation: HashMap::new(),
        }
    }

    fn aggregate(&self) -> ResearchUsageTotals {
        let mut total = ResearchUsageTotals::default();
        for usage in self.by_invocation.values() {
            total.add(*usage);
        }
        total
    }
}

impl ServerRuntime {
    pub(crate) async fn handle_research_turn_start(
        self: &Arc<Self>,
        connection_id: Option<u64>,
        request_id: serde_json::Value,
        params: TurnStartParams,
        display_input: String,
        question: String,
    ) -> serde_json::Value {
        let question = question.trim().to_string();
        if question.is_empty() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::EmptyInput,
                "research question is empty",
            );
        }
        let Some(session_arc) = self.sessions.lock().await.get(&params.session_id).cloned() else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };

        let now = Utc::now();
        let (effective_cwd, runtime_context) = {
            let session = session_arc.lock().await;
            let effective_cwd = params
                .cwd
                .clone()
                .unwrap_or_else(|| session.summary.cwd.clone());
            let runtime_context = if params
                .cwd
                .as_ref()
                .is_some_and(|cwd| cwd != &session.summary.cwd)
            {
                None
            } else {
                Some(Arc::clone(&session.runtime_context))
            };
            (effective_cwd, runtime_context)
        };
        let runtime_context = match runtime_context {
            Some(runtime_context) => runtime_context,
            None => match self.deps.context_for_workspace(&effective_cwd).await {
                Ok(runtime_context) => runtime_context,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InternalError,
                        format!("failed to initialize session workspace: {error}"),
                    );
                }
            },
        };
        let mut cwd_change = None;
        let (turn, turn_config, effective_cwd) = {
            let mut session = session_arc.lock().await;
            if session.active_turn.is_some() {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::TurnAlreadyRunning,
                    "cannot start research while a turn is already running",
                );
            }
            let requested_model = requested_model_selection(
                params.model_binding_id.as_deref(),
                params.model.as_deref(),
                &session.summary,
            );
            let requested_reasoning_effort_selection = params
                .reasoning_effort_selection
                .clone()
                .or_else(|| session.summary.reasoning_effort_selection.clone());
            let turn_config = runtime_context.resolve_turn_config(
                requested_model,
                requested_reasoning_effort_selection.clone(),
            );
            let effective_cwd = params
                .cwd
                .clone()
                .unwrap_or_else(|| session.summary.cwd.clone());
            if let Some(cwd) = params.cwd.clone() {
                let old_cwd = session.summary.cwd.clone();
                if old_cwd != cwd {
                    cwd_change = Some((old_cwd, cwd.clone()));
                    session.runtime_context = Arc::clone(&runtime_context);
                }
                session.summary.cwd = cwd.clone();
                session.core_session.lock().await.cwd = cwd;
            }
            if let Some(permission_mode) = params
                .approval_policy
                .as_deref()
                .and_then(permission_mode_from_approval_policy)
            {
                session.core_session.lock().await.config.permission_mode = permission_mode;
                session.config.permission_mode = permission_mode;
            }
            let resolved_request = turn_config.model.resolve_reasoning_effort_selection(
                turn_config.reasoning_effort_selection.as_deref(),
            );
            let request_model = turn_config.provider_request_model(&resolved_request.request_model);
            apply_turn_config_to_session_summary(&mut session.summary, &turn_config);
            let turn = TurnMetadata {
                turn_id: TurnId::new(),
                session_id: params.session_id,
                sequence: session
                    .latest_turn
                    .as_ref()
                    .map_or(1, |turn| turn.sequence + 1),
                status: TurnStatus::Running,
                kind: devo_core::TurnKind::Research,
                model: turn_config.model.slug.clone(),
                model_binding_id: turn_config.model_binding_id.clone(),
                reasoning_effort_selection: turn_config.reasoning_effort_selection.clone(),
                reasoning_effort: resolved_request.effective_reasoning_effort,
                request_model,
                request_thinking: resolved_request.request_thinking,
                started_at: now,
                completed_at: None,
                usage: None,
                stop_reason: None,
                failure_reason: None,
            };
            session.summary.status = SessionRuntimeStatus::ActiveTurn;
            session.summary.updated_at = now;
            session.summary.last_activity_at = now;
            session.active_turn = Some(turn.clone());
            (turn, turn_config, effective_cwd.display().to_string())
        };

        if let Some((old_cwd, new_cwd)) = cwd_change {
            self.run_session_hook(
                params.session_id,
                devo_core::HookEvent::CwdChanged,
                serde_json::Map::from_iter([
                    (
                        "old_cwd".to_string(),
                        serde_json::Value::String(old_cwd.display().to_string()),
                    ),
                    (
                        "new_cwd".to_string(),
                        serde_json::Value::String(new_cwd.display().to_string()),
                    ),
                ]),
            )
            .await;
        }

        self.active_turn_cancellations
            .lock()
            .await
            .insert(params.session_id, CancellationToken::new());
        if let Some(connection_id) = connection_id {
            self.active_turn_connections
                .lock()
                .await
                .insert(params.session_id, connection_id);
        }
        let research_display_input = research_display_input(&display_input);
        self.maybe_assign_provisional_title(params.session_id, &research_display_input)
            .await;
        {
            let mut session = session_arc.lock().await;
            if session.first_user_input.is_none() {
                session.first_user_input = Some(research_display_input.clone());
            }
        }
        let needs_title = {
            let session = session_arc.lock().await;
            let first_input = session.first_user_input.clone();
            let needs = matches!(
                session.summary.title_state,
                SessionTitleState::Unset | SessionTitleState::Provisional
            );
            (needs, first_input)
        };
        if needs_title.0
            && let Some(first_input) = needs_title.1
        {
            let runtime = Arc::clone(self);
            let sid = params.session_id;
            tokio::spawn(async move {
                runtime.maybe_generate_final_title(sid, first_input).await;
            });
        }
        let (record, session_context, turn_context) = {
            let session = session_arc.lock().await;
            let core_session = session.core_session.lock().await;
            (
                session.record.clone(),
                core_session.session_context.clone(),
                core_session.latest_turn_context.clone(),
            )
        };
        if let Some(record) = record
            && let Err(error) = self.rollout_store.append_turn(
                &record,
                build_turn_record(&turn, session_context, turn_context),
            )
        {
            self.clear_active_turn_runtime_handles(params.session_id)
                .await;
            {
                let mut session = session_arc.lock().await;
                if session
                    .active_turn
                    .as_ref()
                    .is_some_and(|active| active.turn_id == turn.turn_id)
                {
                    session.active_turn = None;
                    session.summary.status = SessionRuntimeStatus::Idle;
                    session.summary.updated_at = Utc::now();
                    session.summary.last_activity_at = session.summary.updated_at;
                }
            }
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to persist research turn start: {error}"),
            );
        }

        self.broadcast_event(ServerEvent::SessionStatusChanged(
            SessionStatusChangedPayload {
                session_id: params.session_id,
                status: SessionRuntimeStatus::ActiveTurn,
            },
        ))
        .await;
        self.broadcast_event(ServerEvent::InputQueueUpdated(
            devo_core::InputQueueUpdatedPayload {
                session_id: params.session_id,
                pending_count: 0,
                pending_texts: vec![],
            },
        ))
        .await;
        self.broadcast_event(ServerEvent::TurnStarted(TurnEventPayload {
            session_id: params.session_id,
            turn: turn.clone(),
        }))
        .await;

        let runtime = Arc::clone(self);
        let turn_for_task = turn.clone();
        let display_input_for_task = research_display_input.clone();
        let runtime_context_for_task = Arc::clone(&runtime_context);
        let task = tokio::spawn(async move {
            runtime
                .execute_research_turn(ExecuteResearchTurnInput {
                    session_id: params.session_id,
                    turn: turn_for_task,
                    runtime_context: runtime_context_for_task,
                    turn_config,
                    display_input: display_input_for_task,
                    question,
                    cwd: effective_cwd,
                })
                .await;
        });
        self.active_tasks
            .lock()
            .await
            .insert(params.session_id, task.abort_handle());

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: TurnStartResult::Started {
                turn_id: turn.turn_id,
                status: turn.status.clone(),
                accepted_at: now,
            },
        })
        .expect("serialize research turn/start response")
    }

    async fn execute_research_turn(self: Arc<Self>, input: ExecuteResearchTurnInput) {
        let ExecuteResearchTurnInput {
            session_id,
            turn,
            runtime_context,
            turn_config,
            display_input,
            question,
            cwd,
        } = input;
        self.capture_turn_workspace_baseline(session_id, turn.turn_id, PathBuf::from(cwd.clone()))
            .await;
        let usage_ledger = self.research_usage_ledger(session_id).await;
        let result = self
            .run_research_pipeline(
                session_id,
                &turn,
                ResearchPipelineInput {
                    runtime_context,
                    turn_config: turn_config.clone(),
                    display_input: &display_input,
                    question: &question,
                    cwd,
                    usage_ledger: Arc::clone(&usage_ledger),
                },
            )
            .await;
        if result.is_err() {
            Arc::clone(&self)
                .close_research_child_agents(session_id)
                .await;
        } else {
            self.clear_research_child_agents(session_id).await;
        }
        let final_usage = usage_ledger.lock().await.aggregate();
        self.clear_active_turn_runtime_handles(session_id).await;

        match result {
            Ok(()) => {
                self.finish_research_turn(session_id, turn, TurnStatus::Completed, final_usage)
                    .await;
            }
            Err(error) => {
                let failure_message = format!("Research failed: {error}");
                self.emit_turn_item(
                    session_id,
                    turn.turn_id,
                    ItemKind::ResearchArtifact,
                    TurnItem::ResearchArtifact(ResearchArtifactItem {
                        artifact_type: ResearchArtifactType::Failure,
                        title: "Research Failure".to_string(),
                        content: failure_message.clone(),
                    }),
                    serde_json::json!({
                        "artifact_type": "failure",
                        "title": "Research Failure",
                        "content": failure_message
                    }),
                )
                .await;
                self.refresh_core_session_prompt_context(session_id).await;
                self.finish_research_turn(session_id, turn, TurnStatus::Failed, final_usage)
                    .await;
            }
        }
    }

    async fn run_research_pipeline(
        self: &Arc<Self>,
        session_id: SessionId,
        turn: &TurnMetadata,
        input: ResearchPipelineInput<'_>,
    ) -> anyhow::Result<()> {
        let ResearchPipelineInput {
            runtime_context,
            turn_config,
            display_input,
            question,
            cwd,
            usage_ledger,
        } = input;
        let model_runtime = ResearchModelRuntime {
            runtime_context: Arc::clone(&runtime_context),
            turn_config: &turn_config,
            usage_ledger: &usage_ledger,
            session_id,
            turn_id: turn.turn_id,
        };
        let research_config = runtime_context
            .config_store
            .lock()
            .expect("app config store mutex should not be poisoned")
            .effective_config()
            .research
            .clone();
        let date = devo_core::research::prompts::today_string();
        let timezone = devo_core::research::prompts::timezone_string();
        let mut research_context =
            ResearchRequestContext::new(question, date.clone(), timezone, cwd);
        self.emit_turn_item(
            session_id,
            turn.turn_id,
            ItemKind::UserMessage,
            TurnItem::UserMessage(TextItem {
                text: display_input.to_string(),
            }),
            serde_json::json!({ "title": "You", "text": display_input }),
        )
        .await;

        let mut coordinator_scratch = self.scratch_session(session_id).await?;
        for message in research_context.session_messages(Vec::new()) {
            coordinator_scratch.push_message(message);
        }

        let clarification_result = self
            .run_clarification_gate(&model_runtime, &mut coordinator_scratch)
            .await?;
        self.emit_research_artifact(
            session_id,
            turn.turn_id,
            ResearchArtifactType::Clarification,
            "Research Clarification",
            clarification_result.artifact_content,
        )
        .await;
        research_context
            .clarifications
            .extend(clarification_result.clarifications.clone());
        for clarification in &clarification_result.clarifications {
            coordinator_scratch.push_message(devo_core::Message::user(
                devo_core::research::prompts::clarification_context(
                    &clarification.question,
                    &clarification.answer,
                ),
            ));
        }

        let research_brief = self
            .run_research_artifact_stage(
                &model_runtime,
                &mut coordinator_scratch,
                ResearchStageKind::Brief,
            )
            .await?;
        coordinator_scratch.push_message(devo_core::Message::user(
            devo_core::research::prompts::research_brief_context(&research_brief),
        ));

        let supervisor_output = self
            .run_supervisor_stage(&model_runtime, &mut coordinator_scratch)
            .await?;
        coordinator_scratch.push_message(devo_core::Message::user(
            devo_core::research::prompts::research_notes_context(&supervisor_output.notes),
        ));
        coordinator_scratch.push_message(devo_core::Message::user(
            devo_core::research::prompts::webpage_summaries_context(""),
        ));

        let compressed = self
            .run_research_artifact_stage(
                &model_runtime,
                &mut coordinator_scratch,
                ResearchStageKind::Compress,
            )
            .await?;
        let compressed_findings = vec![compressed];

        let final_report = self
            .stream_final_report(
                &model_runtime,
                question,
                research_context.session_messages(vec![
                    devo_core::research::prompts::research_brief_context(&research_brief),
                    devo_core::research::prompts::findings_context(
                        &compressed_findings.join("\n\n"),
                    ),
                ]),
            )
            .await?;
        let context_reference = build_research_context_reference(
            question,
            &final_report,
            &compressed_findings,
            supervisor_output.worker_count,
            research_config.max_summary_chars,
        );
        self.emit_research_artifact(
            session_id,
            turn.turn_id,
            ResearchArtifactType::FinalReportMetadata,
            "Research Context Reference",
            context_reference,
        )
        .await;
        self.refresh_core_session_prompt_context(session_id).await;
        Ok(())
    }

    async fn request_research_clarification(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        question: &str,
    ) -> anyhow::Result<String> {
        let request_id = format!("research_clarification_{turn_id}");
        let (tx, rx) = tokio::sync::oneshot::channel();
        let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() else {
            anyhow::bail!("session does not exist");
        };
        {
            let mut session = session_arc.lock().await;
            session
                .pending_user_inputs
                .insert(request_id.clone(), PendingUserInput { turn_id, tx });
            session.summary.status = SessionRuntimeStatus::WaitingClient;
        }
        self.broadcast_event(ServerEvent::SessionStatusChanged(
            SessionStatusChangedPayload {
                session_id,
                status: SessionRuntimeStatus::WaitingClient,
            },
        ))
        .await;
        self.broadcast_event(ServerEvent::RequestUserInput(RequestUserInputPayload {
            request: crate::PendingServerRequestContext {
                request_id: request_id.clone().into(),
                request_kind: ServerRequestKind::ResearchClarificationRequest,
                session_id,
                turn_id: Some(turn_id),
                item_id: None,
            },
            questions: vec![RequestUserInputQuestion {
                id: "clarification".to_string(),
                header: "Research".to_string(),
                question: question.to_string(),
                is_other: true,
                is_secret: false,
                options: None,
            }],
        }))
        .await;
        let response = rx.await?;
        if let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() {
            let mut session = session_arc.lock().await;
            session.summary.status = SessionRuntimeStatus::ActiveTurn;
        }
        self.broadcast_event(ServerEvent::SessionStatusChanged(
            SessionStatusChangedPayload {
                session_id,
                status: SessionRuntimeStatus::ActiveTurn,
            },
        ))
        .await;
        Ok(response
            .answers
            .get("clarification")
            .and_then(|answer| answer.answers.first())
            .cloned()
            .unwrap_or_default())
    }

    async fn run_research_artifact_stage(
        self: &Arc<Self>,
        runtime: &ResearchModelRuntime<'_>,
        scratch: &mut SessionState,
        stage: ResearchStageKind,
    ) -> anyhow::Result<String> {
        let mut capture = ResearchArtifactQueryCapture::default();
        self.run_research_stage_query(
            runtime,
            scratch,
            stage,
            ResearchStageCapture::Artifact(&mut capture),
        )
        .await?;
        self.complete_reasoning_item(runtime.session_id, runtime.turn_id, &mut capture.reasoning)
            .await;
        if !capture.turn_completed {
            anyhow::bail!("research {stage:?} stream ended without message completion");
        }
        let text = if capture.text.trim().is_empty() {
            assistant_text_from_session(scratch)
        } else {
            capture.text.clone()
        };
        let artifact = stage
            .artifact()
            .ok_or_else(|| anyhow::anyhow!("research {stage:?} has no artifact"))?;
        self.complete_research_artifact_item(
            runtime.session_id,
            runtime.turn_id,
            &mut capture.artifact,
            &artifact,
            &text,
        )
        .await;
        Ok(text)
    }

    async fn run_supervisor_stage(
        self: &Arc<Self>,
        runtime: &ResearchModelRuntime<'_>,
        scratch: &mut SessionState,
    ) -> anyhow::Result<SupervisorOutput> {
        let mut capture = SupervisorQueryCapture::default();
        self.run_research_stage_query(
            runtime,
            scratch,
            ResearchStageKind::Supervisor,
            ResearchStageCapture::Supervisor(&mut capture),
        )
        .await?;
        self.complete_reasoning_item(runtime.session_id, runtime.turn_id, &mut capture.reasoning)
            .await;

        let supervisor_text = if capture.text.trim().is_empty() {
            assistant_text_from_session(scratch)
        } else {
            capture.text.clone()
        };
        let notes = if supervisor_text.trim().is_empty() {
            "Supervisor completed without visible notes. Treat evidence as unavailable unless structured worker tool output is present."
                .to_string()
        } else {
            supervisor_text
        };
        let artifact = ResearchStageKind::Supervisor
            .artifact()
            .expect("supervisor stage should have an artifact");
        self.complete_research_artifact_item(
            runtime.session_id,
            runtime.turn_id,
            &mut capture.artifact,
            &artifact,
            &notes,
        )
        .await;

        Ok(SupervisorOutput {
            notes,
            worker_count: capture.spawned_worker_count,
        })
    }

    async fn run_research_stage_query(
        self: &Arc<Self>,
        runtime: &ResearchModelRuntime<'_>,
        scratch: &mut SessionState,
        stage: ResearchStageKind,
        mut capture: ResearchStageCapture<'_>,
    ) -> anyhow::Result<()> {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let callback = Arc::new(move |event: QueryEvent| {
            let _ = tx.send(event);
        });
        let mut stage_turn_config = runtime.turn_config.clone();
        stage_turn_config.web_search = devo_core::ResolvedWebSearchConfig::Disabled;
        stage_turn_config.web_fetch = devo_core::ResolvedWebFetchConfig::Disabled;
        scratch.config.token_budget = stage_turn_config.token_budget();
        scratch.session_context = Some(research_session_context(
            scratch,
            &stage_turn_config,
            research_stage_system(stage.prompt()),
        ));
        let registry = Arc::new(
            runtime
                .runtime_context
                .registry
                .restricted_to_specs(stage.tool_names()),
        );
        let tool_runtime = self
            .tool_runtime_for_research(
                runtime.session_id,
                runtime.turn_id,
                &stage_turn_config,
                Arc::clone(&registry),
            )
            .await?;
        let artifact = stage.artifact();
        let query_result = {
            let query_future = query(
                scratch,
                &stage_turn_config,
                runtime
                    .runtime_context
                    .provider_for_route(stage_turn_config.provider_route.clone()),
                Arc::clone(&registry),
                &tool_runtime,
                Some(callback),
            );
            tokio::pin!(query_future);
            let mut event_channel_closed = false;
            loop {
                tokio::select! {
                    maybe_event = rx.recv(), if !event_channel_closed => {
                        if let Some(event) = maybe_event {
                            let context = ResearchQueryEventContext {
                                session_id: runtime.session_id,
                                turn_id: runtime.turn_id,
                                usage_ledger: runtime.usage_ledger,
                                context_window: Some(stage_turn_config.model.context_window as u64),
                            };
                            self.handle_research_stage_query_event(
                                context,
                                stage,
                                &mut capture,
                                artifact.as_ref(),
                                event,
                            )
                            .await?;
                        } else {
                            event_channel_closed = true;
                        }
                    }
                    result = &mut query_future => {
                        break result;
                    }
                }
            }
        };
        query_result?;
        while let Some(event) = rx.recv().await {
            let context = ResearchQueryEventContext {
                session_id: runtime.session_id,
                turn_id: runtime.turn_id,
                usage_ledger: runtime.usage_ledger,
                context_window: Some(stage_turn_config.model.context_window as u64),
            };
            self.handle_research_stage_query_event(
                context,
                stage,
                &mut capture,
                artifact.as_ref(),
                event,
            )
            .await?;
        }
        Ok(())
    }

    async fn handle_research_stage_query_event(
        &self,
        context: ResearchQueryEventContext<'_>,
        stage: ResearchStageKind,
        capture: &mut ResearchStageCapture<'_>,
        artifact: Option<&StreamedResearchArtifact>,
        event: QueryEvent,
    ) -> anyhow::Result<()> {
        match capture {
            ResearchStageCapture::Clarification(capture) => {
                self.handle_clarification_query_event(context, capture, event)
                    .await;
            }
            ResearchStageCapture::Artifact(capture) => {
                let artifact = artifact
                    .ok_or_else(|| anyhow::anyhow!("research {stage:?} missing artifact"))?;
                let artifact_context = ResearchArtifactEventContext {
                    query: context,
                    artifact,
                };
                self.handle_research_artifact_query_event(artifact_context, stage, capture, event)
                    .await;
            }
            ResearchStageCapture::Supervisor(capture) => {
                let artifact = artifact
                    .ok_or_else(|| anyhow::anyhow!("research supervisor missing artifact"))?;
                let artifact_context = ResearchArtifactEventContext {
                    query: context,
                    artifact,
                };
                self.handle_supervisor_query_event(artifact_context, capture, event)
                    .await;
            }
            ResearchStageCapture::FinalReport(capture) => {
                self.handle_final_report_query_event(context, capture, event)
                    .await;
            }
        }
        Ok(())
    }

    async fn handle_research_artifact_query_event(
        &self,
        context: ResearchArtifactEventContext<'_>,
        stage: ResearchStageKind,
        capture: &mut ResearchArtifactQueryCapture,
        event: QueryEvent,
    ) {
        let session_id = context.query.session_id;
        let turn_id = context.query.turn_id;
        let usage_ledger = context.query.usage_ledger;
        let context_window = context.query.context_window;
        match event {
            QueryEvent::TextDelta(text) => {
                capture.text.push_str(&text);
                self.push_research_artifact_delta(
                    session_id,
                    turn_id,
                    &mut capture.artifact,
                    context.artifact,
                    text,
                )
                .await;
            }
            QueryEvent::Usage { usage } => {
                let usage_key = format!(
                    "{}_{}",
                    stage.usage_prefix(),
                    capture.usage_invocation_index
                );
                self.apply_research_usage(
                    session_id,
                    turn_id,
                    usage_ledger,
                    usage_key,
                    ResearchUsageTotals::from_usage(&usage),
                    context_window,
                )
                .await;
                capture.usage_invocation_index += 1;
            }
            QueryEvent::UsageDelta { usage } => {
                let usage_key = format!(
                    "{}_{}",
                    stage.usage_prefix(),
                    capture.usage_invocation_index
                );
                self.apply_research_usage(
                    session_id,
                    turn_id,
                    usage_ledger,
                    usage_key,
                    ResearchUsageTotals::from_usage(&usage),
                    context_window,
                )
                .await;
            }
            QueryEvent::ReasoningDelta(text) => {
                self.push_reasoning_delta(session_id, turn_id, &mut capture.reasoning, text)
                    .await;
            }
            QueryEvent::ReasoningCompleted => {
                self.complete_reasoning_item(session_id, turn_id, &mut capture.reasoning)
                    .await;
            }
            QueryEvent::TurnComplete { .. } => {
                capture.turn_completed = true;
            }
            QueryEvent::ToolUseStart { .. }
            | QueryEvent::ToolExecutionStart { .. }
            | QueryEvent::ToolProgress { .. }
            | QueryEvent::ToolResult { .. } => {}
        }
    }

    async fn handle_supervisor_query_event(
        &self,
        context: ResearchArtifactEventContext<'_>,
        capture: &mut SupervisorQueryCapture,
        event: QueryEvent,
    ) {
        let session_id = context.query.session_id;
        let turn_id = context.query.turn_id;
        let usage_ledger = context.query.usage_ledger;
        let context_window = context.query.context_window;
        match event {
            QueryEvent::TextDelta(text) => {
                capture.text.push_str(&text);
                self.push_research_artifact_delta(
                    session_id,
                    turn_id,
                    &mut capture.artifact,
                    context.artifact,
                    text,
                )
                .await;
            }
            QueryEvent::ToolUseStart { id, name, input } => {
                let (item_id, item_seq) = self
                    .start_item(
                        session_id,
                        turn_id,
                        ItemKind::ToolCall,
                        serde_json::to_value(ToolCallPayload {
                            tool_call_id: id.clone(),
                            tool_name: name.clone(),
                            parameters: input.clone(),
                            command_actions: Vec::new(),
                        })
                        .expect("serialize supervisor tool call payload"),
                    )
                    .await;
                capture.pending_tools.insert(
                    id,
                    PendingResearchToolCall {
                        item_id,
                        item_seq,
                        tool_name: name,
                        input,
                    },
                );
            }
            QueryEvent::ToolExecutionStart { .. } => {}
            QueryEvent::ToolResult {
                tool_use_id,
                tool_name,
                input,
                content,
                display_content,
                is_error,
                summary,
            } => {
                let output = tool_content_to_json(content);
                if is_spawn_agent_tool_name(&tool_name)
                    && !is_error
                    && let Some(child_session_id) = spawn_agent_child_session_id(&output)
                {
                    self.remember_research_child_agent(session_id, child_session_id)
                        .await;
                    capture.spawned_worker_count += 1;
                }
                if let Some(pending) = capture.pending_tools.remove(&tool_use_id) {
                    self.complete_item(
                        session_id,
                        turn_id,
                        pending.item_id,
                        pending.item_seq,
                        ItemKind::ToolCall,
                        TurnItem::ToolCall(ToolCallItem {
                            tool_call_id: tool_use_id.clone(),
                            tool_name: pending.tool_name.clone(),
                            input: pending.input.clone(),
                        }),
                        serde_json::to_value(ToolCallPayload {
                            tool_call_id: tool_use_id.clone(),
                            tool_name: pending.tool_name,
                            parameters: pending.input,
                            command_actions: Vec::new(),
                        })
                        .expect("serialize completed supervisor tool call"),
                    )
                    .await;
                }
                self.emit_turn_item(
                    session_id,
                    turn_id,
                    ItemKind::ToolResult,
                    TurnItem::ToolResult(ToolResultItem {
                        tool_call_id: tool_use_id.clone(),
                        tool_name: Some(tool_name.clone()),
                        output: output.clone(),
                        display_content: display_content.clone(),
                        is_error,
                    }),
                    serde_json::to_value(ToolResultPayload {
                        tool_call_id: tool_use_id,
                        tool_name: Some(tool_name),
                        input: (!input.is_null()).then_some(input),
                        content: output,
                        display_content,
                        is_error,
                        summary,
                    })
                    .expect("serialize supervisor tool result payload"),
                )
                .await;
            }
            QueryEvent::Usage { usage } => {
                let usage_key = format!("supervisor_call_{}", capture.usage_invocation_index);
                self.apply_research_usage(
                    session_id,
                    turn_id,
                    usage_ledger,
                    usage_key,
                    ResearchUsageTotals::from_usage(&usage),
                    context_window,
                )
                .await;
                capture.usage_invocation_index += 1;
            }
            QueryEvent::UsageDelta { usage } => {
                let usage_key = format!("supervisor_call_{}", capture.usage_invocation_index);
                self.apply_research_usage(
                    session_id,
                    turn_id,
                    usage_ledger,
                    usage_key,
                    ResearchUsageTotals::from_usage(&usage),
                    context_window,
                )
                .await;
            }
            QueryEvent::ReasoningDelta(text) => {
                self.push_reasoning_delta(session_id, turn_id, &mut capture.reasoning, text)
                    .await;
            }
            QueryEvent::ReasoningCompleted => {
                self.complete_reasoning_item(session_id, turn_id, &mut capture.reasoning)
                    .await;
            }
            QueryEvent::TurnComplete { .. } | QueryEvent::ToolProgress { .. } => {}
        }
    }

    async fn remember_research_child_agent(
        &self,
        parent_session_id: SessionId,
        child_session_id: SessionId,
    ) {
        self.research_child_agents
            .lock()
            .await
            .entry(parent_session_id)
            .or_default()
            .insert(child_session_id);
    }

    async fn clear_research_child_agents(&self, parent_session_id: SessionId) {
        self.research_child_agents
            .lock()
            .await
            .remove(&parent_session_id);
    }

    pub(super) async fn close_research_child_agents(self: Arc<Self>, parent_session_id: SessionId) {
        let child_session_ids = self
            .research_child_agents
            .lock()
            .await
            .remove(&parent_session_id)
            .map(|children| children.into_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        for child_session_id in child_session_ids {
            if let Err(error) = Arc::clone(&self)
                .close_agent(devo_protocol::CloseAgentParams {
                    session_id: parent_session_id,
                    target: child_session_id.to_string(),
                })
                .await
            {
                tracing::warn!(
                    parent_session_id = %parent_session_id,
                    child_session_id = %child_session_id,
                    error = %error,
                    "failed to close research child agent"
                );
            }
        }
    }

    async fn handle_final_report_query_event(
        &self,
        context: ResearchQueryEventContext<'_>,
        capture: &mut ResearchQueryCapture,
        event: QueryEvent,
    ) {
        let session_id = context.session_id;
        let turn_id = context.turn_id;
        let usage_ledger = context.usage_ledger;
        let context_window = context.context_window;
        match event {
            QueryEvent::TextDelta(text) => {
                capture.text.push_str(&text);
                self.push_agent_message_delta(session_id, turn_id, &mut capture.assistant, text)
                    .await;
            }
            QueryEvent::ToolUseStart { id, name, input } => {
                let (item_id, item_seq) = self
                    .start_item(
                        session_id,
                        turn_id,
                        ItemKind::ToolCall,
                        serde_json::to_value(ToolCallPayload {
                            tool_call_id: id.clone(),
                            tool_name: name.clone(),
                            parameters: input.clone(),
                            command_actions: Vec::new(),
                        })
                        .expect("serialize final report tool call payload"),
                    )
                    .await;
                capture.pending_tools.insert(
                    id,
                    PendingResearchToolCall {
                        item_id,
                        item_seq,
                        tool_name: name,
                        input,
                    },
                );
            }
            QueryEvent::ToolExecutionStart { .. } => {}
            QueryEvent::ToolResult {
                tool_use_id,
                tool_name,
                input,
                content,
                display_content,
                is_error,
                summary,
            } => {
                let output = tool_content_to_json(content);
                if is_write_tool_name(&tool_name)
                    && !is_error
                    && let Some(path) = extract_written_file_path(&input, &output)
                    && let Some(content) = input
                        .get("content")
                        .and_then(serde_json::Value::as_str)
                        .filter(|content| !content.trim().is_empty())
                {
                    capture.final_report_write = Some(FinalReportWrite {
                        path,
                        content: content.to_string(),
                    });
                }
                if let Some(pending) = capture.pending_tools.remove(&tool_use_id) {
                    self.complete_item(
                        session_id,
                        turn_id,
                        pending.item_id,
                        pending.item_seq,
                        ItemKind::ToolCall,
                        TurnItem::ToolCall(ToolCallItem {
                            tool_call_id: tool_use_id.clone(),
                            tool_name: pending.tool_name.clone(),
                            input: pending.input.clone(),
                        }),
                        serde_json::to_value(ToolCallPayload {
                            tool_call_id: tool_use_id.clone(),
                            tool_name: pending.tool_name,
                            parameters: pending.input,
                            command_actions: Vec::new(),
                        })
                        .expect("serialize completed final report tool call"),
                    )
                    .await;
                }
                self.emit_turn_item(
                    session_id,
                    turn_id,
                    ItemKind::ToolResult,
                    TurnItem::ToolResult(ToolResultItem {
                        tool_call_id: tool_use_id.clone(),
                        tool_name: Some(tool_name.clone()),
                        output: output.clone(),
                        display_content: display_content.clone(),
                        is_error,
                    }),
                    serde_json::to_value(ToolResultPayload {
                        tool_call_id: tool_use_id,
                        tool_name: Some(tool_name),
                        input: (!input.is_null()).then_some(input),
                        content: output,
                        display_content,
                        is_error,
                        summary,
                    })
                    .expect("serialize final report tool result payload"),
                )
                .await;
            }
            QueryEvent::Usage { usage } => {
                let usage_key = format!("final_report_call_{}", capture.usage_invocation_index);
                self.apply_research_usage(
                    session_id,
                    turn_id,
                    usage_ledger,
                    usage_key,
                    ResearchUsageTotals::from_usage(&usage),
                    context_window,
                )
                .await;
                capture.usage_invocation_index += 1;
            }
            QueryEvent::UsageDelta { usage } => {
                let usage_key = format!("final_report_call_{}", capture.usage_invocation_index);
                self.apply_research_usage(
                    session_id,
                    turn_id,
                    usage_ledger,
                    usage_key,
                    ResearchUsageTotals::from_usage(&usage),
                    context_window,
                )
                .await;
            }
            QueryEvent::ReasoningDelta(text) => {
                self.push_reasoning_delta(session_id, turn_id, &mut capture.reasoning, text)
                    .await;
            }
            QueryEvent::ReasoningCompleted => {
                self.complete_reasoning_item(session_id, turn_id, &mut capture.reasoning)
                    .await;
            }
            QueryEvent::TurnComplete { .. } => {
                capture.turn_completed = true;
            }
            QueryEvent::ToolProgress { .. } => {}
        }
    }

    async fn handle_clarification_query_event(
        &self,
        context: ResearchQueryEventContext<'_>,
        capture: &mut ClarificationQueryCapture,
        event: QueryEvent,
    ) {
        let session_id = context.session_id;
        let turn_id = context.turn_id;
        let usage_ledger = context.usage_ledger;
        let context_window = context.context_window;
        match event {
            QueryEvent::TextDelta(text) => {
                capture.text.push_str(&text);
            }
            QueryEvent::ToolUseStart {
                id, name, input, ..
            } => {
                if is_request_user_input_tool_name(&name) {
                    let questions = request_user_input_questions_from_input(&input);
                    if !questions.is_empty() {
                        capture
                            .pending_request_user_input_questions
                            .insert(id, questions);
                    }
                }
            }
            QueryEvent::ToolResult {
                tool_use_id,
                tool_name,
                content,
                ..
            } => {
                if is_request_user_input_tool_name(&tool_name) {
                    let output = tool_content_to_json(content);
                    if let Ok(response) =
                        serde_json::from_value::<devo_protocol::RequestUserInputResponse>(output)
                    {
                        let questions = capture
                            .pending_request_user_input_questions
                            .remove(&tool_use_id)
                            .unwrap_or_default();
                        let exchanges =
                            request_user_input_exchanges_from_response(&questions, &response);
                        capture.clarifications.extend(
                            exchanges
                                .iter()
                                .filter(|exchange| !exchange.answer.trim().is_empty())
                                .cloned(),
                        );
                        capture.request_user_input_exchanges.extend(exchanges);
                    }
                }
            }
            QueryEvent::Usage { usage } => {
                let usage_key = format!("clarify_call_{}", capture.usage_invocation_index);
                self.apply_research_usage(
                    session_id,
                    turn_id,
                    usage_ledger,
                    usage_key,
                    ResearchUsageTotals::from_usage(&usage),
                    context_window,
                )
                .await;
                capture.usage_invocation_index += 1;
            }
            QueryEvent::UsageDelta { usage } => {
                let usage_key = format!("clarify_call_{}", capture.usage_invocation_index);
                self.apply_research_usage(
                    session_id,
                    turn_id,
                    usage_ledger,
                    usage_key,
                    ResearchUsageTotals::from_usage(&usage),
                    context_window,
                )
                .await;
            }
            QueryEvent::ReasoningDelta(text) => {
                self.push_reasoning_delta(session_id, turn_id, &mut capture.reasoning, text)
                    .await;
            }
            QueryEvent::ReasoningCompleted => {
                self.complete_reasoning_item(session_id, turn_id, &mut capture.reasoning)
                    .await;
            }
            QueryEvent::TurnComplete { .. }
            | QueryEvent::ToolExecutionStart { .. }
            | QueryEvent::ToolProgress { .. } => {}
        }
    }

    async fn emit_research_artifact(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        artifact_type: ResearchArtifactType,
        title: impl Into<String>,
        content: impl Into<String>,
    ) {
        let artifact = ResearchArtifactItem {
            artifact_type,
            title: title.into(),
            content: content.into(),
        };
        self.emit_turn_item(
            session_id,
            turn_id,
            ItemKind::ResearchArtifact,
            TurnItem::ResearchArtifact(artifact.clone()),
            serde_json::to_value(artifact).expect("serialize research artifact item"),
        )
        .await;
    }

    async fn refresh_core_session_prompt_context(&self, session_id: SessionId) {
        let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() else {
            return;
        };
        let (persisted_turn_items, latest_compaction_snapshot, core_session) = {
            let session = session_arc.lock().await;
            (
                session.persisted_turn_items.clone(),
                session.latest_compaction_snapshot.clone(),
                Arc::clone(&session.core_session),
            )
        };

        let mut rebuilt_messages = Vec::new();
        let mut ignored_history_items = Vec::new();
        let mut tool_names_by_id = HashMap::new();
        for item in &persisted_turn_items {
            crate::persistence::apply_turn_item(
                &mut rebuilt_messages,
                &mut ignored_history_items,
                &mut tool_names_by_id,
                &item.turn_kind,
                item.turn_item.clone(),
            );
        }
        let rebuilt_prompt_messages = latest_compaction_snapshot.as_ref().and_then(|snapshot| {
            crate::persistence::build_prompt_messages_from_snapshot(&persisted_turn_items, snapshot)
        });

        let mut core_session = core_session.lock().await;
        core_session.messages = rebuilt_messages;
        core_session.prompt_messages = rebuilt_prompt_messages;
    }

    async fn push_agent_message_delta(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        state: &mut StreamedTextItem,
        delta: String,
    ) {
        if delta.is_empty() {
            return;
        }
        let item_id = match (state.item_id, state.item_seq) {
            (Some(item_id), Some(_)) => item_id,
            (None, None) => {
                let (item_id, item_seq) = self
                    .start_item(
                        session_id,
                        turn_id,
                        ItemKind::AgentMessage,
                        serde_json::json!({ "title": "Assistant", "text": "" }),
                    )
                    .await;
                state.item_id = Some(item_id);
                state.item_seq = Some(item_seq);
                item_id
            }
            _ => return,
        };
        state.text.push_str(&delta);
        self.broadcast_event(ServerEvent::ItemDelta {
            delta_kind: ItemDeltaKind::AgentMessageDelta,
            payload: ItemDeltaPayload {
                context: EventContext {
                    session_id,
                    turn_id: Some(turn_id),
                    item_id: Some(item_id),
                    seq: 0,
                },
                delta,
                stream_index: None,
                channel: None,
            },
        })
        .await;
    }

    async fn complete_agent_message_item(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        state: &mut StreamedTextItem,
        final_text: String,
    ) {
        if state.item_id.is_none() && !final_text.trim().is_empty() {
            let (item_id, item_seq) = self
                .start_item(
                    session_id,
                    turn_id,
                    ItemKind::AgentMessage,
                    serde_json::json!({ "title": "Assistant", "text": "" }),
                )
                .await;
            state.item_id = Some(item_id);
            state.item_seq = Some(item_seq);
        }
        let (Some(item_id), Some(item_seq)) = (state.item_id.take(), state.item_seq.take()) else {
            return;
        };
        self.complete_item(
            session_id,
            turn_id,
            item_id,
            item_seq,
            ItemKind::AgentMessage,
            TurnItem::AgentMessage(TextItem {
                text: final_text.clone(),
            }),
            serde_json::json!({ "title": "Assistant", "text": final_text }),
        )
        .await;
    }

    async fn push_reasoning_delta(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        state: &mut StreamedTextItem,
        delta: String,
    ) {
        if delta.is_empty() {
            return;
        }
        let item_id = match (state.item_id, state.item_seq) {
            (Some(item_id), Some(_)) => item_id,
            (None, None) => {
                let (item_id, item_seq) = self
                    .start_item(
                        session_id,
                        turn_id,
                        ItemKind::Reasoning,
                        serde_json::json!({ "title": "Reasoning", "text": "" }),
                    )
                    .await;
                state.item_id = Some(item_id);
                state.item_seq = Some(item_seq);
                item_id
            }
            _ => return,
        };
        state.text.push_str(&delta);
        self.broadcast_event(ServerEvent::ItemDelta {
            delta_kind: ItemDeltaKind::ReasoningTextDelta,
            payload: ItemDeltaPayload {
                context: EventContext {
                    session_id,
                    turn_id: Some(turn_id),
                    item_id: Some(item_id),
                    seq: 0,
                },
                delta,
                stream_index: None,
                channel: None,
            },
        })
        .await;
    }

    async fn complete_reasoning_item(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        state: &mut StreamedTextItem,
    ) {
        let (Some(item_id), Some(item_seq)) = (state.item_id.take(), state.item_seq.take()) else {
            return;
        };
        let text = std::mem::take(&mut state.text);
        self.complete_item(
            session_id,
            turn_id,
            item_id,
            item_seq,
            ItemKind::Reasoning,
            TurnItem::Reasoning(TextItem { text: text.clone() }),
            serde_json::json!({ "title": "Reasoning", "text": text }),
        )
        .await;
    }

    async fn run_clarification_gate(
        self: &Arc<Self>,
        runtime: &ResearchModelRuntime<'_>,
        scratch: &mut SessionState,
    ) -> anyhow::Result<ClarificationGateResult> {
        let mut capture = ClarificationQueryCapture::default();
        self.run_research_stage_query(
            runtime,
            scratch,
            ResearchStageKind::Clarify,
            ResearchStageCapture::Clarification(&mut capture),
        )
        .await?;
        self.complete_reasoning_item(runtime.session_id, runtime.turn_id, &mut capture.reasoning)
            .await;

        if !capture.request_user_input_exchanges.is_empty() {
            return Ok(ClarificationGateResult {
                artifact_content: clarification_artifact_content(
                    &capture.request_user_input_exchanges,
                ),
                clarifications: capture.clarifications,
            });
        }

        let clarify_text = if capture.text.trim().is_empty() {
            assistant_text_from_session(scratch)
        } else {
            capture.text.clone()
        };
        if let Some(decision) = parse_json_object::<ClarifyDecision>(&clarify_text) {
            if decision.need_clarification && !decision.question.trim().is_empty() {
                let question = decision.question;
                let answer = self
                    .request_research_clarification(runtime.session_id, runtime.turn_id, &question)
                    .await?;
                let artifact_content =
                    format!("Question: {}\n\nAnswer: {}", question, answer.trim());
                let clarifications = if answer.trim().is_empty() {
                    Vec::new()
                } else {
                    vec![ResearchClarificationContext { question, answer }]
                };
                return Ok(ClarificationGateResult {
                    artifact_content,
                    clarifications,
                });
            }
            let artifact_content = if decision.verification.trim().is_empty() {
                "No clarification needed.".to_string()
            } else {
                decision.verification
            };
            return Ok(ClarificationGateResult {
                artifact_content,
                clarifications: Vec::new(),
            });
        }

        let artifact_content = if clarify_text.trim().is_empty() {
            "No clarification needed.".to_string()
        } else {
            clarify_text
        };
        Ok(ClarificationGateResult {
            artifact_content,
            clarifications: Vec::new(),
        })
    }

    async fn stream_final_report(
        self: &Arc<Self>,
        runtime: &ResearchModelRuntime<'_>,
        question: &str,
        messages: Vec<devo_core::Message>,
    ) -> anyhow::Result<String> {
        let mut scratch = self.scratch_session(runtime.session_id).await?;
        for message in messages {
            scratch.push_message(message);
        }
        let mut capture = ResearchQueryCapture::default();
        self.run_research_stage_query(
            runtime,
            &mut scratch,
            ResearchStageKind::FinalReport,
            ResearchStageCapture::FinalReport(&mut capture),
        )
        .await?;
        let mut final_turn_config = runtime.turn_config.clone();
        final_turn_config.web_search = devo_core::ResolvedWebSearchConfig::Disabled;
        final_turn_config.web_fetch = devo_core::ResolvedWebFetchConfig::Disabled;
        let registry = Arc::new(
            runtime
                .runtime_context
                .registry
                .restricted_to_specs(RESEARCH_FILE_TOOL_NAMES),
        );
        let tool_runtime = self
            .tool_runtime_for_research(
                runtime.session_id,
                runtime.turn_id,
                &final_turn_config,
                Arc::clone(&registry),
            )
            .await?;
        self.complete_reasoning_item(runtime.session_id, runtime.turn_id, &mut capture.reasoning)
            .await;
        if !capture.turn_completed {
            anyhow::bail!("research final report stream ended without message completion");
        }
        let mut final_text = capture.text.clone();
        if final_text.trim().is_empty() {
            final_text = scratch
                .messages
                .iter()
                .rev()
                .find(|message| message.role == devo_core::Role::Assistant)
                .map(|message| {
                    message
                        .content
                        .iter()
                        .filter_map(|block| match block {
                            devo_core::ContentBlock::Text { text } => Some(text.as_str()),
                            devo_core::ContentBlock::Reasoning { .. }
                            | devo_core::ContentBlock::ProviderReasoning { .. }
                            | devo_core::ContentBlock::ToolUse { .. }
                            | devo_core::ContentBlock::HostedToolUse { .. }
                            | devo_core::ContentBlock::ToolResult { .. } => None,
                        })
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default();
        }
        let report_file_requested = final_report_file_requested_by_default(question);
        let mut final_report_write = capture.final_report_write.clone();
        let report_text = final_report_write
            .as_ref()
            .map(|write| write.content.clone())
            .filter(|content| !content.trim().is_empty())
            .unwrap_or_else(|| final_text.clone());
        if report_text.trim().is_empty() {
            anyhow::bail!("research final report stream completed without report text");
        }
        if report_file_requested && final_report_write.is_none() {
            let path = self
                .write_final_report_fallback(
                    runtime.session_id,
                    runtime.turn_id,
                    &tool_runtime,
                    question,
                    &report_text,
                )
                .await?;
            final_report_write = Some(FinalReportWrite {
                path,
                content: report_text.clone(),
            });
        }
        let completed_text = final_report_write
            .as_ref()
            .filter(|_| report_file_requested || final_text.trim().is_empty())
            .map(|write| final_report_written_response(&write.path, &report_text))
            .unwrap_or(final_text);
        self.complete_agent_message_item(
            runtime.session_id,
            runtime.turn_id,
            &mut capture.assistant,
            completed_text,
        )
        .await;
        Ok(report_text)
    }

    async fn write_final_report_fallback(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        runtime: &ToolRuntime,
        question: &str,
        report_text: &str,
    ) -> anyhow::Result<String> {
        let tool_call_id = format!("final_report_write_{turn_id}");
        let file_path = final_report_file_name(question);
        let input = serde_json::json!({
            "filePath": file_path,
            "content": report_text,
        });
        let (item_id, item_seq) = self
            .start_item(
                session_id,
                turn_id,
                ItemKind::ToolCall,
                serde_json::to_value(ToolCallPayload {
                    tool_call_id: tool_call_id.clone(),
                    tool_name: "write".to_string(),
                    parameters: input.clone(),
                    command_actions: Vec::new(),
                })
                .expect("serialize fallback final report write tool call"),
            )
            .await;
        let call = ToolCall {
            id: tool_call_id.clone(),
            name: "write".to_string(),
            input: input.clone(),
        };
        let mut results = runtime.execute_batch(&[call]).await;
        let Some(result) = results.pop() else {
            anyhow::bail!("fallback final report write produced no tool result");
        };
        self.complete_item(
            session_id,
            turn_id,
            item_id,
            item_seq,
            ItemKind::ToolCall,
            TurnItem::ToolCall(ToolCallItem {
                tool_call_id: tool_call_id.clone(),
                tool_name: "write".to_string(),
                input: input.clone(),
            }),
            serde_json::to_value(ToolCallPayload {
                tool_call_id: tool_call_id.clone(),
                tool_name: "write".to_string(),
                parameters: input.clone(),
                command_actions: Vec::new(),
            })
            .expect("serialize completed fallback final report write tool call"),
        )
        .await;
        let output = tool_content_to_json(result.content.clone());
        let display_content = result.display_content.clone();
        let summary = display_content
            .clone()
            .unwrap_or_else(|| "write final report".to_string());
        self.emit_turn_item(
            session_id,
            turn_id,
            ItemKind::ToolResult,
            TurnItem::ToolResult(ToolResultItem {
                tool_call_id: tool_call_id.clone(),
                tool_name: Some("write".to_string()),
                output: output.clone(),
                display_content: display_content.clone(),
                is_error: result.is_error,
            }),
            serde_json::to_value(ToolResultPayload {
                tool_call_id,
                tool_name: Some("write".to_string()),
                input: Some(input.clone()),
                content: output.clone(),
                display_content,
                is_error: result.is_error,
                summary,
            })
            .expect("serialize fallback final report write tool result"),
        )
        .await;
        if result.is_error {
            anyhow::bail!(
                "fallback final report write failed: {}",
                result.content.into_string()
            );
        }
        extract_written_file_path(&input, &output)
            .or_else(|| {
                input
                    .get("filePath")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
            .ok_or_else(|| {
                anyhow::anyhow!("fallback final report write did not report a file path")
            })
    }

    async fn push_research_artifact_delta(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        state: &mut StreamedTextItem,
        artifact: &StreamedResearchArtifact,
        delta: String,
    ) {
        if delta.is_empty() {
            return;
        }
        let item_id = match (state.item_id, state.item_seq) {
            (Some(item_id), Some(_)) => item_id,
            (None, None) => {
                let (item_id, item_seq) = self
                    .start_item(
                        session_id,
                        turn_id,
                        ItemKind::ResearchArtifact,
                        serde_json::to_value(ResearchArtifactItem {
                            artifact_type: artifact.artifact_type.clone(),
                            title: artifact.title.clone(),
                            content: String::new(),
                        })
                        .expect("serialize streamed research artifact"),
                    )
                    .await;
                state.item_id = Some(item_id);
                state.item_seq = Some(item_seq);
                item_id
            }
            _ => return,
        };
        state.text.push_str(&delta);
        self.broadcast_event(ServerEvent::ItemDelta {
            delta_kind: ItemDeltaKind::ResearchArtifactDelta,
            payload: ItemDeltaPayload {
                context: EventContext {
                    session_id,
                    turn_id: Some(turn_id),
                    item_id: Some(item_id),
                    seq: 0,
                },
                delta,
                stream_index: None,
                channel: None,
            },
        })
        .await;
    }

    async fn complete_research_artifact_item(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        state: &mut StreamedTextItem,
        artifact: &StreamedResearchArtifact,
        final_text: &str,
    ) {
        if state.item_id.is_none() && !final_text.trim().is_empty() {
            let (item_id, item_seq) = self
                .start_item(
                    session_id,
                    turn_id,
                    ItemKind::ResearchArtifact,
                    serde_json::to_value(ResearchArtifactItem {
                        artifact_type: artifact.artifact_type.clone(),
                        title: artifact.title.clone(),
                        content: String::new(),
                    })
                    .expect("serialize streamed research artifact"),
                )
                .await;
            state.item_id = Some(item_id);
            state.item_seq = Some(item_seq);
        }
        let (Some(item_id), Some(item_seq)) = (state.item_id.take(), state.item_seq.take()) else {
            return;
        };
        let content = if final_text.trim().is_empty() {
            std::mem::take(&mut state.text)
        } else {
            final_text.to_string()
        };
        self.complete_item(
            session_id,
            turn_id,
            item_id,
            item_seq,
            ItemKind::ResearchArtifact,
            TurnItem::ResearchArtifact(ResearchArtifactItem {
                artifact_type: artifact.artifact_type.clone(),
                title: artifact.title.clone(),
                content: content.clone(),
            }),
            serde_json::to_value(ResearchArtifactItem {
                artifact_type: artifact.artifact_type.clone(),
                title: artifact.title.clone(),
                content,
            })
            .expect("serialize completed research artifact"),
        )
        .await;
    }

    async fn scratch_session(&self, session_id: SessionId) -> anyhow::Result<SessionState> {
        let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() else {
            anyhow::bail!("session does not exist");
        };
        let session = session_arc.lock().await;
        let core_session = session.core_session.lock().await;
        let mut scratch = SessionState::new(core_session.config.clone(), core_session.cwd.clone());
        scratch.id = session_id.to_string();
        Ok(scratch)
    }

    async fn tool_runtime_for_research(
        self: &Arc<Self>,
        session_id: SessionId,
        turn_id: TurnId,
        turn_config: &TurnConfig,
        registry: Arc<ToolRegistry>,
    ) -> anyhow::Result<ToolRuntime> {
        let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() else {
            anyhow::bail!("session does not exist");
        };
        let (cwd, permission_mode, permission_profile, runtime_context) = {
            let session = session_arc.lock().await;
            let core_session = session.core_session.lock().await;
            (
                core_session.cwd.clone(),
                core_session.config.permission_mode,
                core_session.config.permission_profile.clone(),
                Arc::clone(&session.runtime_context),
            )
        };
        let provider_http = runtime_context
            .config_store
            .lock()
            .expect("app config store mutex should not be poisoned")
            .effective_config()
            .provider_http
            .clone();
        let turn_cancel_token = self
            .active_turn_cancellations
            .lock()
            .await
            .get(&session_id)
            .cloned()
            .unwrap_or_else(CancellationToken::new);
        let tool_execution_start_runtime = Arc::clone(self);
        Ok(ToolRuntime::new_with_context_and_options(
            registry,
            self.build_permission_checker(session_id, turn_id, permission_mode, permission_profile),
            ToolRuntimeContext {
                session_id: session_id.to_string(),
                turn_id: Some(turn_id.to_string()),
                cwd,
                agent_scope: ToolAgentScope::Parent,
                agent_context_mode: devo_protocol::AgentContextMode::DeepResearch,
                collaboration_mode: devo_protocol::CollaborationMode::Build,
                agent_coordinator: Some(Arc::clone(self) as Arc<dyn AgentToolCoordinator>),
                client_filesystem: Some(Arc::clone(self) as Arc<dyn ClientFilesystem>),
                client_terminal: Some(Arc::clone(self) as Arc<dyn ClientTerminal>),
                local_web_search: match &turn_config.web_search {
                    devo_core::ResolvedWebSearchConfig::Local(config) => Some(config.clone()),
                    devo_core::ResolvedWebSearchConfig::Disabled
                    | devo_core::ResolvedWebSearchConfig::Provider => None,
                },
                hooks: self.hook_context_for_session(session_id).await,
                network_proxy: provider_http.proxy_url,
                network_no_proxy: provider_http.no_proxy,
            },
            ToolExecutionOptions {
                cancel_token: turn_cancel_token,
                on_tool_execution_start: Some(Arc::new(move |call: &ToolCall| {
                    let runtime = Arc::clone(&tool_execution_start_runtime);
                    let tool_call_id = call.id.clone();
                    tokio::spawn(async move {
                        runtime
                            .broadcast_event(ServerEvent::ToolCallStatusUpdated(
                                devo_protocol::ToolCallStatusUpdatedPayload {
                                    session_id,
                                    turn_id,
                                    tool_call_id,
                                    status: "in_progress".to_string(),
                                    terminal_id: None,
                                },
                            ))
                            .await;
                    });
                })),
                ..ToolExecutionOptions::default()
            },
        ))
    }

    async fn finish_research_turn(
        self: &Arc<Self>,
        session_id: SessionId,
        mut turn: TurnMetadata,
        status: TurnStatus,
        final_usage: ResearchUsageTotals,
    ) {
        turn.status = status.clone();
        turn.completed_at = Some(Utc::now());
        let usage = final_usage.to_turn_usage();
        {
            let session_arc = self.sessions.lock().await.get(&session_id).cloned();
            if let Some(session_arc) = session_arc {
                let mut session = session_arc.lock().await;
                turn.usage = Some(usage.clone());
                session.latest_turn = Some(turn.clone());
                session.active_turn = None;
                session.summary.status = SessionRuntimeStatus::Idle;
                session.summary.updated_at = Utc::now();
                session.summary.last_activity_at = session.summary.updated_at;
            }
        }
        let (record, session_context, turn_context) = {
            let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() else {
                return;
            };
            let session = session_arc.lock().await;
            let core_session = session.core_session.lock().await;
            (
                session.record.clone(),
                core_session.session_context.clone(),
                core_session.latest_turn_context.clone(),
            )
        };
        if let Some(record) = record
            && let Err(error) = self.rollout_store.append_turn(
                &record,
                build_turn_record(&turn, session_context, turn_context),
            )
        {
            tracing::warn!(session_id = %session_id, error = %error, "failed to persist research turn finish");
        }
        self.finalize_turn_workspace_changes(session_id, &turn)
            .await;
        match status {
            TurnStatus::Completed => {
                self.broadcast_event(ServerEvent::TurnCompleted(TurnEventPayload {
                    session_id,
                    turn,
                }))
                .await;
            }
            TurnStatus::Failed => {
                self.broadcast_event(ServerEvent::TurnFailed(TurnEventPayload {
                    session_id,
                    turn: turn.clone(),
                }))
                .await;
                self.broadcast_event(ServerEvent::TurnCompleted(TurnEventPayload {
                    session_id,
                    turn,
                }))
                .await;
            }
            TurnStatus::Interrupted
            | TurnStatus::Running
            | TurnStatus::Pending
            | TurnStatus::WaitingApproval => {}
        }
        self.broadcast_event(ServerEvent::SessionStatusChanged(
            SessionStatusChangedPayload {
                session_id,
                status: SessionRuntimeStatus::Idle,
            },
        ))
        .await;
        self.spawn_next_turn_from_queue(session_id).await;
        self.maybe_start_goal_continuation_turn(session_id).await;
    }

    async fn apply_research_usage(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        usage_ledger: &ResearchUsageLedgerRef,
        usage_key: String,
        usage: ResearchUsageTotals,
        context_window: Option<u64>,
    ) {
        let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() else {
            return;
        };
        let (base, aggregate) = {
            let mut ledger = usage_ledger.lock().await;
            ledger.by_invocation.insert(usage_key, usage);
            (ledger.base, ledger.aggregate())
        };
        let (total_input_tokens, total_output_tokens, total_tokens, total_cache_read_tokens) = {
            let mut session = session_arc.lock().await;
            session.summary.total_input_tokens = base.input_tokens + aggregate.input_tokens;
            session.summary.total_output_tokens = base.output_tokens + aggregate.output_tokens;
            session.summary.total_tokens = base.total_tokens + aggregate.total_tokens;
            session.summary.total_cache_creation_tokens =
                base.cache_creation_input_tokens + aggregate.cache_creation_input_tokens;
            session.summary.total_cache_read_tokens =
                base.cache_read_input_tokens + aggregate.cache_read_input_tokens;
            session.summary.last_query_total_tokens = aggregate.total_tokens;
            (
                session.summary.total_input_tokens,
                session.summary.total_output_tokens,
                session.summary.total_tokens,
                session.summary.total_cache_read_tokens,
            )
        };
        self.broadcast_event(ServerEvent::TurnUsageUpdated(TurnUsageUpdatedPayload {
            session_id,
            turn_id,
            usage: aggregate.to_turn_usage(),
            total_input_tokens,
            total_output_tokens,
            total_tokens,
            total_cache_read_tokens,
            last_query_input_tokens: aggregate.input_tokens,
            context_window,
        }))
        .await;
    }

    async fn research_usage_ledger(&self, session_id: SessionId) -> ResearchUsageLedgerRef {
        let base = if let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() {
            let session = session_arc.lock().await;
            ResearchUsageTotals {
                input_tokens: session.summary.total_input_tokens,
                output_tokens: session.summary.total_output_tokens,
                total_tokens: session.summary.total_tokens,
                cache_creation_input_tokens: session.summary.total_cache_creation_tokens,
                cache_read_input_tokens: session.summary.total_cache_read_tokens,
                reasoning_output_tokens: 0,
            }
        } else {
            ResearchUsageTotals::default()
        };
        Arc::new(Mutex::new(ResearchUsageLedger::new(base)))
    }
}

fn parse_json_object<T>(text: &str) -> Option<T>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_str(text).ok().or_else(|| {
        let start = text.find('{')?;
        let end = text.rfind('}')?;
        serde_json::from_str(&text[start..=end]).ok()
    })
}

fn assistant_text_from_session(session: &SessionState) -> String {
    session
        .messages
        .iter()
        .rev()
        .find(|message| message.role == devo_core::Role::Assistant)
        .map(|message| {
            message
                .content
                .iter()
                .filter_map(|block| match block {
                    devo_core::ContentBlock::Text { text } => Some(text.as_str()),
                    devo_core::ContentBlock::Reasoning { .. }
                    | devo_core::ContentBlock::ProviderReasoning { .. }
                    | devo_core::ContentBlock::ToolUse { .. }
                    | devo_core::ContentBlock::HostedToolUse { .. }
                    | devo_core::ContentBlock::ToolResult { .. } => None,
                })
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

fn is_request_user_input_tool_name(tool_name: &str) -> bool {
    matches!(tool_name, "request_user_input" | "question")
}

fn clarification_artifact_content(exchanges: &[ResearchClarificationContext]) -> String {
    match exchanges {
        [] => "No clarification needed.".to_string(),
        [exchange] => format!(
            "Question: {}\n\nAnswer: {}",
            exchange.question,
            exchange.answer.trim()
        ),
        _ => exchanges
            .iter()
            .enumerate()
            .map(|(index, exchange)| {
                let item = index + 1;
                format!(
                    "Question {item}: {}\n\nAnswer {item}: {}",
                    exchange.question,
                    exchange.answer.trim()
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n"),
    }
}

fn request_user_input_questions_from_input(input: &serde_json::Value) -> Vec<(String, String)> {
    if let Some(question) = input
        .get("question")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|question| !question.is_empty())
    {
        return vec![("question".to_string(), question.to_string())];
    }

    let Some(questions) = input.get("questions").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };

    questions
        .iter()
        .filter_map(|question| {
            let id = question
                .get("id")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())?;
            let question_text = question
                .get("question")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|question| !question.is_empty())?;
            Some((id.to_string(), question_text.to_string()))
        })
        .collect()
}

fn request_user_input_exchanges_from_response(
    questions: &[(String, String)],
    response: &devo_protocol::RequestUserInputResponse,
) -> Vec<ResearchClarificationContext> {
    questions
        .iter()
        .filter_map(|(id, question)| {
            let answer = response.answers.get(id)?;
            Some(ResearchClarificationContext {
                question: question.clone(),
                answer: first_non_empty_request_user_input_answer(answer).unwrap_or_default(),
            })
        })
        .collect()
}

fn first_non_empty_request_user_input_answer(
    answer: &devo_protocol::RequestUserInputAnswer,
) -> Option<String> {
    answer
        .answers
        .iter()
        .find(|text| !text.trim().is_empty())
        .cloned()
}

fn build_research_context_reference(
    question: &str,
    final_report: &str,
    compressed_findings: &[String],
    task_count: usize,
    max_chars: usize,
) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let mut reference = format!(
        "Original question:\n{}\n\nResearch workers: {}",
        question.trim(),
        task_count
    );
    let source_hints = collect_reference_hints(final_report, compressed_findings, 8);
    if !source_hints.is_empty() {
        reference.push_str("\n\nSource/reference hints:\n");
        reference.push_str(&source_hints.join("\n"));
    }
    truncate_chars(&reference, max_chars)
}

fn collect_reference_hints(
    final_report: &str,
    compressed_findings: &[String],
    max_hints: usize,
) -> Vec<String> {
    let mut hints = Vec::new();
    for text in std::iter::once(final_report).chain(compressed_findings.iter().map(String::as_str))
    {
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let lower = trimmed.to_ascii_lowercase();
            let looks_like_reference = trimmed.contains("http://")
                || trimmed.contains("https://")
                || lower.starts_with("source")
                || lower.starts_with("sources")
                || lower.starts_with("citation")
                || lower.starts_with("citations");
            if !looks_like_reference {
                continue;
            }
            let mut line_hints = extract_urls(trimmed);
            if line_hints.is_empty()
                && (lower.starts_with("source")
                    || lower.starts_with("sources")
                    || lower.starts_with("citation")
                    || lower.starts_with("citations"))
            {
                line_hints.push(truncate_chars(trimmed, 300));
            }
            for hint in line_hints {
                if !hints.contains(&hint) {
                    hints.push(hint);
                }
                if hints.len() >= max_hints {
                    return hints;
                }
            }
        }
    }
    hints
}

fn extract_urls(text: &str) -> Vec<String> {
    text.split_whitespace()
        .filter(|part| part.starts_with("http://") || part.starts_with("https://"))
        .map(|part| {
            part.trim_end_matches(['.', ',', ';', ')', ']', '}'])
                .to_string()
        })
        .filter(|url| !url.is_empty())
        .collect()
}

fn is_spawn_agent_tool_name(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "spawn_agent" | "spawn_subagent" | "subagent" | "delegate"
    )
}

fn spawn_agent_child_session_id(output: &serde_json::Value) -> Option<SessionId> {
    serde_json::from_value::<devo_protocol::SpawnAgentResult>(output.clone())
        .ok()
        .map(|result| result.child_session_id)
}

#[cfg(test)]
fn structured_tool_evidence_messages(
    messages: &[devo_core::Message],
) -> Vec<devo_protocol::RequestMessage> {
    messages
        .iter()
        .filter_map(|message| {
            let content = message
                .content
                .iter()
                .filter_map(structured_tool_evidence_content)
                .collect::<Vec<_>>();
            if content.is_empty() {
                None
            } else {
                Some(devo_protocol::RequestMessage {
                    role: message.role.as_str().to_string(),
                    content,
                })
            }
        })
        .collect()
}

#[cfg(test)]
fn structured_tool_evidence_content(
    block: &devo_core::ContentBlock,
) -> Option<devo_protocol::RequestContent> {
    match block {
        devo_core::ContentBlock::ProviderReasoning { provider, payload } => {
            Some(devo_protocol::RequestContent::ProviderReasoning {
                provider: provider.clone(),
                payload: payload.clone(),
            })
        }
        devo_core::ContentBlock::ToolUse { id, name, input } => {
            Some(devo_protocol::RequestContent::ToolUse {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
            })
        }
        devo_core::ContentBlock::HostedToolUse {
            id,
            name,
            input,
            output,
            status,
        } => Some(devo_protocol::RequestContent::HostedToolUse {
            id: id.clone(),
            name: name.clone(),
            input: input.clone(),
            output: output.clone(),
            status: status.clone(),
        }),
        devo_core::ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => Some(devo_protocol::RequestContent::ToolResult {
            tool_use_id: tool_use_id.clone(),
            content: content.clone(),
            is_error: (*is_error).then_some(true),
        }),
        devo_core::ContentBlock::Text { .. } | devo_core::ContentBlock::Reasoning { .. } => None,
    }
}

pub(crate) fn research_stage_system(stage_prompt: String) -> String {
    let mut system = devo_core::research::prompts::system();
    if !stage_prompt.trim().is_empty() {
        system.push_str("\n\n");
        system.push_str(stage_prompt.trim());
    }
    system
}

pub(crate) fn research_session_context(
    session: &SessionState,
    turn_config: &TurnConfig,
    system_prompt: String,
) -> devo_core::SessionContext {
    let model = &turn_config.model;
    let reasoning_effort_selection = turn_config.reasoning_effort_selection.as_deref();
    let normalized_reasoning_effort_selection =
        model.normalize_reasoning_effort_selection(reasoning_effort_selection);
    let resolved =
        model.resolve_reasoning_effort_selection(normalized_reasoning_effort_selection.as_deref());
    devo_core::SessionContext {
        base_instructions: system_prompt,
        available_skills: None,
        workspace_instructions: None,
        locked_agents_snapshot: None,
        environment: devo_core::EnvironmentContext::capture(&session.cwd),
        language: devo_core::LanguageContext::default(),
        persona: devo_core::Persona::Default,
        model: model.clone(),
        reasoning_effort_selection: normalized_reasoning_effort_selection,
        reasoning_effort: resolved.effective_reasoning_effort,
        system_prompt_mode: devo_core::SystemPromptMode::DeepResearch,
    }
}

fn final_report_file_requested_by_default(question: &str) -> bool {
    let question = question.to_ascii_lowercase();
    ![
        "inline-only",
        "inline only",
        "in chat only",
        "chat only",
        "no local file",
        "no file",
        "do not write",
        "don't write",
        "without writing",
        "do not create",
        "don't create",
    ]
    .iter()
    .any(|phrase| question.contains(phrase))
}

fn final_report_file_name(question: &str) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;
    for ch in question.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            previous_dash = false;
        } else if !previous_dash && !slug.is_empty() {
            slug.push('-');
            previous_dash = true;
        }
        if slug.len() >= 64 {
            break;
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "research-report.md".to_string()
    } else {
        format!("{slug}.md")
    }
}

fn final_report_written_response(path: &str, report_text: &str) -> String {
    let summary = report_text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.trim_start_matches('#').trim())
        .filter(|line| !line.is_empty())
        .unwrap_or("Research report completed.");
    format!("Wrote the full research report to `{path}`.\n\n{summary}")
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    if max_chars <= 14 {
        return text.chars().take(max_chars).collect();
    }
    let mut truncated = text
        .chars()
        .take(max_chars.saturating_sub(14))
        .collect::<String>();
    truncated.push_str("\n[truncated]");
    truncated
}

fn research_display_input(display_input: &str) -> String {
    let trimmed = display_input.trim();
    if trimmed == "/research" || trimmed.starts_with("/research ") {
        trimmed.to_string()
    } else {
        format!("/research {trimmed}")
    }
}

fn tool_content_to_json(content: ToolContent) -> serde_json::Value {
    match content {
        ToolContent::Text(text) => serde_json::Value::String(text),
        ToolContent::Json(json) => json,
        ToolContent::Mixed { text, json } => {
            json.unwrap_or_else(|| serde_json::Value::String(text.unwrap_or_default()))
        }
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::*;

    #[test]
    fn request_user_input_exchanges_follow_question_order_and_ignore_unknown_answers() {
        // Trace: L2-DES-RESEARCH-001
        // Verifies: clarification tool answers map back to the original ordered questions.
        let questions = request_user_input_questions_from_input(&json!({
            "questions": [
                {"id": "scope", "question": "Which scope?"},
                {"id": "region", "question": "Which region?"},
                {"id": "empty", "question": "Optional detail?"}
            ]
        }));
        let response = serde_json::from_value::<devo_protocol::RequestUserInputResponse>(json!({
            "answers": {
                "region": {"answers": ["APAC"]},
                "unknown": {"answers": ["ignore me"]},
                "scope": {"answers": ["Product docs"]},
                "empty": {"answers": ["   "]}
            }
        }))
        .expect("request_user_input response should deserialize");

        let exchanges = request_user_input_exchanges_from_response(&questions, &response);

        assert_eq!(
            exchanges,
            vec![
                ResearchClarificationContext {
                    question: "Which scope?".to_string(),
                    answer: "Product docs".to_string(),
                },
                ResearchClarificationContext {
                    question: "Which region?".to_string(),
                    answer: "APAC".to_string(),
                },
                ResearchClarificationContext {
                    question: "Optional detail?".to_string(),
                    answer: String::new(),
                },
            ]
        );
        assert_eq!(
            exchanges
                .iter()
                .filter(|exchange| !exchange.answer.trim().is_empty())
                .cloned()
                .collect::<Vec<_>>(),
            vec![
                ResearchClarificationContext {
                    question: "Which scope?".to_string(),
                    answer: "Product docs".to_string(),
                },
                ResearchClarificationContext {
                    question: "Which region?".to_string(),
                    answer: "APAC".to_string(),
                },
            ]
        );
    }

    #[test]
    fn clarification_artifact_content_numbers_multiple_questions() {
        // Trace: L2-DES-RESEARCH-001
        // Verifies: multiple clarification exchanges render deterministically for transcript artifacts.
        let content = clarification_artifact_content(&[
            ResearchClarificationContext {
                question: "Which scope?".to_string(),
                answer: "Product docs".to_string(),
            },
            ResearchClarificationContext {
                question: "Optional detail?".to_string(),
                answer: String::new(),
            },
        ]);

        assert_eq!(
            content,
            "Question 1: Which scope?\n\nAnswer 1: Product docs\n\nQuestion 2: Optional detail?\n\nAnswer 2: "
        );
    }

    #[test]
    fn structured_tool_evidence_messages_preserve_hosted_pairs_without_text() {
        // Trace: L2-DES-RESEARCH-001
        // Verifies: research compression can receive provider-hosted tool context as structured blocks.
        let messages = vec![devo_core::Message {
            role: devo_core::Role::Assistant,
            content: vec![
                devo_core::ContentBlock::Text {
                    text: "visible notes stay in research_notes".to_string(),
                },
                devo_core::ContentBlock::HostedToolUse {
                    id: "hosted_ws_1".to_string(),
                    name: "web_search".to_string(),
                    input: json!({"query": "DeepSeek official website"}),
                    output: None,
                    status: None,
                },
                devo_core::ContentBlock::HostedToolUse {
                    id: "hosted_ws_1".to_string(),
                    name: "web_search".to_string(),
                    input: json!({"query": "DeepSeek official website"}),
                    output: Some(json!([{
                        "title": "DeepSeek",
                        "url": "https://www.deepseek.com/"
                    }])),
                    status: Some("completed".to_string()),
                },
            ],
        }];

        let evidence = structured_tool_evidence_messages(&messages);

        assert_eq!(
            serde_json::to_value(&evidence).expect("serialize evidence messages"),
            json!([
                {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "hosted_tool_use",
                            "id": "hosted_ws_1",
                            "name": "web_search",
                            "input": {"query": "DeepSeek official website"}
                        },
                        {
                            "type": "hosted_tool_use",
                            "id": "hosted_ws_1",
                            "name": "web_search",
                            "input": {"query": "DeepSeek official website"},
                            "output": [{
                                "title": "DeepSeek",
                                "url": "https://www.deepseek.com/"
                            }],
                            "status": "completed"
                        }
                    ]
                }
            ])
        );
    }

    #[test]
    fn research_context_reference_keeps_source_hints_without_evidence_pack_text() {
        // Trace: L2-DES-RESEARCH-001
        // Verifies: follow-up coding turns receive a compact research handoff instead of internal artifacts.
        let reference = build_research_context_reference(
            "What changed?",
            "Final answer cites https://example.com/a and includes a concise conclusion.",
            &[String::from(
                "Internal evidence pack.\nSource: https://example.com/b\nHidden notes should only appear if room remains.",
            )],
            2,
            1_000,
        );

        assert_eq!(
            reference,
            "Original question:\nWhat changed?\n\nResearch workers: 2\n\nSource/reference hints:\nhttps://example.com/a\nhttps://example.com/b"
        );
    }
}

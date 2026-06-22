use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use devo_core::SessionState;
use devo_protocol::RequestUserInputQuestion;
use devo_protocol::ResponseExtra;
use devo_protocol::ServerRequestKind;
use devo_protocol::StreamEvent;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use serde::Deserialize;
use tokio::sync::Semaphore;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::session_context::SessionRuntimeContext;

pub(crate) const RESEARCH_FILE_TOOL_NAMES: &[&str] = &["read", "write", "apply_patch"];
pub(crate) const RESEARCH_WORKER_TOOL_NAMES: &[&str] =
    &["read", "write", "apply_patch", "web_search", "webfetch"];
const RESEARCH_SUBAGENT_CONTINUATION_ATTEMPTS: usize = 1;
const RESEARCH_SUBAGENT_RESTART_ATTEMPTS: usize = 1;

#[derive(Debug, Clone, Deserialize)]
struct ClarifyDecision {
    need_clarification: bool,
    #[serde(default)]
    question: String,
    #[serde(default)]
    verification: String,
}

#[derive(Debug, Clone, Deserialize)]
struct SupervisorPlan {
    tasks: Vec<ResearchTask>,
}

#[derive(Debug, Clone, Deserialize)]
struct ResearchTask {
    title: String,
    research_topic: String,
    #[serde(default)]
    purpose: String,
    #[serde(default)]
    source_strategy: String,
    #[serde(default)]
    success_criteria: String,
}

#[derive(Debug, Clone)]
struct ResearcherOutput {
    title: String,
    topic: String,
    notes: String,
    structured_evidence_messages: Vec<devo_protocol::RequestMessage>,
    oversized_fetches: Vec<OversizedFetch>,
}

#[derive(Debug, Clone)]
struct ResearchRequestContext {
    question: String,
    current_date: String,
    timezone: String,
    cwd: String,
    clarification: Option<ResearchClarificationContext>,
}

#[derive(Debug, Clone)]
struct ResearchClarificationContext {
    question: String,
    answer: String,
}

impl ResearchRequestContext {
    fn new(question: &str, current_date: String, timezone: String, cwd: String) -> Self {
        Self {
            question: question.to_string(),
            current_date,
            timezone,
            cwd,
            clarification: None,
        }
    }

    fn set_clarification(&mut self, question: String, answer: String) {
        self.clarification = Some(ResearchClarificationContext { question, answer });
    }

    fn request_messages(
        &self,
        additional_context: Vec<String>,
    ) -> Vec<devo_protocol::RequestMessage> {
        self.context_texts(additional_context)
            .into_iter()
            .map(request_text_message)
            .collect()
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
        if let Some(clarification) = &self.clarification {
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

#[derive(Debug, Clone)]
struct OversizedFetch {
    content: String,
    source_url: String,
    source_title: String,
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

#[derive(Debug, Clone)]
enum ResearchModelTextDisplay {
    Hidden,
    ResearchArtifact(StreamedResearchArtifact),
}

#[derive(Debug, Clone)]
struct ResearchSubagentTerminal {
    status: String,
    detail: Option<String>,
    completed: bool,
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

struct ResearcherAgentTaskInput<'a> {
    research_brief: &'a str,
    research_context: &'a ResearchRequestContext,
    task_index: usize,
    total_tasks: usize,
}

struct ResearchChildWaitTarget<'a> {
    session_id: SessionId,
    turn_id: TurnId,
    artifact_item_id: ItemId,
    child_session_id: SessionId,
    agent_path: &'a str,
}

struct ResearchChildWaitBuffers<'a> {
    after_sequence: &'a mut u64,
    notes: &'a mut String,
}

type ResearchUsageLedgerRef = Arc<Mutex<ResearchUsageLedger>>;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ResearchUsageTotals {
    input_tokens: usize,
    output_tokens: usize,
    cache_creation_input_tokens: usize,
    cache_read_input_tokens: usize,
}

impl ResearchUsageTotals {
    fn from_usage(usage: &devo_protocol::Usage) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_creation_input_tokens: usage.cache_creation_input_tokens.unwrap_or(0),
            cache_read_input_tokens: usage.cache_read_input_tokens.unwrap_or(0),
        }
    }

    fn from_parts(
        input_tokens: usize,
        output_tokens: usize,
        cache_creation_input_tokens: Option<usize>,
        cache_read_input_tokens: Option<usize>,
    ) -> Self {
        Self {
            input_tokens,
            output_tokens,
            cache_creation_input_tokens: cache_creation_input_tokens.unwrap_or(0),
            cache_read_input_tokens: cache_read_input_tokens.unwrap_or(0),
        }
    }

    fn add(&mut self, other: Self) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_creation_input_tokens += other.cache_creation_input_tokens;
        self.cache_read_input_tokens += other.cache_read_input_tokens;
    }

    fn to_turn_usage(self) -> TurnUsage {
        TurnUsage {
            input_tokens: self.input_tokens as u32,
            output_tokens: self.output_tokens as u32,
            cache_creation_input_tokens: (self.cache_creation_input_tokens > 0)
                .then_some(self.cache_creation_input_tokens as u32),
            cache_read_input_tokens: (self.cache_read_input_tokens > 0)
                .then_some(self.cache_read_input_tokens as u32),
        }
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
            if matches!(
                turn_config.web_search,
                devo_core::ResolvedWebSearchConfig::Disabled
            ) {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    "/research requires web_search to be enabled",
                );
            }
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
            self.active_turn_cancellations
                .lock()
                .await
                .remove(&params.session_id);
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
        self.active_tasks.lock().await.remove(&session_id);
        self.active_turn_cancellations
            .lock()
            .await
            .remove(&session_id);

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

        let clarify_prompt = devo_core::research::prompts::clarify();
        let clarify_text = self
            .model_text(
                &model_runtime,
                clarify_prompt,
                research_context.request_messages(Vec::new()),
                "clarify".to_string(),
            )
            .await?;
        if let Some(decision) = parse_json_object::<ClarifyDecision>(&clarify_text) {
            if decision.need_clarification && !decision.question.trim().is_empty() {
                let answer = self
                    .request_research_clarification(session_id, turn.turn_id, &decision.question)
                    .await?;
                self.emit_research_artifact(
                    session_id,
                    turn.turn_id,
                    ResearchArtifactType::Clarification,
                    "Research Clarification",
                    format!(
                        "Question: {}\n\nAnswer: {}",
                        decision.question,
                        answer.trim()
                    ),
                )
                .await;
                if !answer.trim().is_empty() {
                    research_context.set_clarification(decision.question, answer);
                }
            } else {
                let content = if decision.verification.trim().is_empty() {
                    "No clarification needed.".to_string()
                } else {
                    decision.verification
                };
                self.emit_research_artifact(
                    session_id,
                    turn.turn_id,
                    ResearchArtifactType::Clarification,
                    "Research Clarification",
                    content,
                )
                .await;
            }
        } else {
            self.emit_research_artifact(
                session_id,
                turn.turn_id,
                ResearchArtifactType::Clarification,
                "Research Clarification",
                "No clarification needed.",
            )
            .await;
        }

        let brief_prompt = devo_core::research::prompts::research_brief();
        let research_brief = self
            .model_text_artifact(
                &model_runtime,
                brief_prompt,
                research_context.request_messages(Vec::new()),
                "brief".to_string(),
                ResearchArtifactType::Brief,
                "Research Brief",
            )
            .await?;

        let plan_prompt = devo_core::research::prompts::supervisor(research_config.max_tasks);
        let plan_text = self
            .model_text(
                &model_runtime,
                plan_prompt,
                research_context.request_messages(vec![
                    devo_core::research::prompts::research_brief_context(&research_brief),
                ]),
                "plan".to_string(),
            )
            .await?;
        let mut tasks = parse_json_object::<SupervisorPlan>(&plan_text)
            .map(|plan| plan.tasks)
            .unwrap_or_default();
        if tasks.is_empty() {
            tasks.push(ResearchTask {
                title: "Research".to_string(),
                research_topic: research_brief.clone(),
                purpose: String::new(),
                source_strategy: String::new(),
                success_criteria: String::new(),
            });
        }
        tasks.truncate(research_config.max_tasks.max(1));
        let plan_content = tasks
            .iter()
            .enumerate()
            .map(|(index, task)| {
                let mut task_text = format!(
                    "{}. {}\n{}",
                    index + 1,
                    task.title.trim(),
                    task.research_topic.trim()
                );
                if !task.purpose.trim().is_empty() {
                    task_text.push_str(&format!("\nPurpose: {}", task.purpose.trim()));
                }
                if !task.source_strategy.trim().is_empty() {
                    task_text.push_str(&format!(
                        "\nSource strategy: {}",
                        task.source_strategy.trim()
                    ));
                }
                if !task.success_criteria.trim().is_empty() {
                    task_text.push_str(&format!(
                        "\nSuccess criteria: {}",
                        task.success_criteria.trim()
                    ));
                }
                task_text
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        self.emit_research_artifact(
            session_id,
            turn.turn_id,
            ResearchArtifactType::Plan,
            "Research Plan",
            plan_content,
        )
        .await;

        let total_tasks = tasks.len();
        let task_concurrency = research_config.max_concurrent_tasks.max(1);
        let semaphore = Arc::new(Semaphore::new(task_concurrency));
        let mut research_futures = FuturesUnordered::new();
        for (index, task) in tasks.into_iter().enumerate() {
            let runtime = Arc::clone(self);
            let research_brief = research_brief.clone();
            let research_context = research_context.clone();
            let semaphore = Arc::clone(&semaphore);
            research_futures.push(async move {
                let _permit = semaphore.acquire_owned().await?;
                let output = Arc::clone(&runtime)
                    .run_researcher_agent_task(
                        session_id,
                        turn.turn_id,
                        ResearcherAgentTaskInput {
                            research_brief: &research_brief,
                            research_context: &research_context,
                            task_index: index,
                            total_tasks,
                        },
                        task,
                    )
                    .await?;
                Ok::<(usize, ResearcherOutput), anyhow::Error>((index, output))
            });
        }
        let mut outputs = Vec::new();
        while let Some(output) = research_futures.next().await {
            let output = output?;
            outputs.push(output);
        }
        outputs.sort_by_key(|(index, _)| *index);

        let mut compressed_findings = Vec::new();
        for (output_index, output) in outputs {
            let webpage_summaries = self
                .summarize_oversized_fetches(
                    &model_runtime,
                    &output.topic,
                    output.oversized_fetches,
                    research_config.max_summary_chars,
                    &research_context,
                    format!("researcher_{output_index}"),
                )
                .await?;
            let compress_prompt = devo_core::research::prompts::compress();
            let webpage_summaries = webpage_summaries.join("\n\n");
            let mut compress_messages = research_context.request_messages(vec![
                devo_core::research::prompts::research_topic_context(&output.topic),
                devo_core::research::prompts::research_notes_context(&output.notes),
            ]);
            compress_messages.extend(output.structured_evidence_messages.clone());
            compress_messages.push(request_text_message(
                devo_core::research::prompts::webpage_summaries_context(&webpage_summaries),
            ));
            let compressed = self
                .model_text_artifact(
                    &model_runtime,
                    compress_prompt,
                    compress_messages,
                    format!("researcher_{output_index}_compress"),
                    ResearchArtifactType::CompressedFinding,
                    format!("Compressed Finding: {}", output.title),
                )
                .await?;
            compressed_findings.push(compressed);
        }

        let final_prompt = devo_core::research::prompts::final_report();
        let final_report = self
            .stream_final_report(
                &model_runtime,
                final_prompt,
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
            total_tasks,
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

    async fn run_researcher_agent_task(
        self: Arc<Self>,
        session_id: SessionId,
        turn_id: TurnId,
        input: ResearcherAgentTaskInput<'_>,
        task: ResearchTask,
    ) -> anyhow::Result<ResearcherOutput> {
        let ResearcherAgentTaskInput {
            research_brief,
            research_context,
            task_index,
            total_tasks,
        } = input;
        let artifact_title = format!("Research Finding: {}", task.title.trim());
        let artifact = ResearchArtifactItem {
            artifact_type: ResearchArtifactType::Finding,
            title: artifact_title.clone(),
            content: String::new(),
        };
        let (artifact_item_id, artifact_item_seq) = self
            .start_item(
                session_id,
                turn_id,
                ItemKind::ResearchArtifact,
                serde_json::to_value(&artifact).expect("serialize streamed research artifact"),
            )
            .await;

        let spawn_result = Arc::clone(&self)
            .spawn_agent(devo_protocol::SpawnAgentParams {
                session_id,
                message: delegated_research_task_message(
                    research_brief,
                    research_context,
                    task_index,
                    total_tasks,
                    &task,
                ),
                fork_turns: Some("none".to_string()),
                max_turns: None,
                tool_policy: devo_protocol::AgentToolPolicy::DeepResearch,
                context_mode: devo_protocol::AgentContextMode::DeepResearch,
                ephemeral: false,
            })
            .await?;
        self.remember_research_child_agent(session_id, spawn_result.child_session_id)
            .await;
        let mut child_session_ids = vec![spawn_result.child_session_id];
        let mut terminal_agent_path = spawn_result.agent_path.clone();

        let mut notes = String::new();
        let mut structured_evidence_messages = Vec::new();
        let mut after_sequence = 0_u64;
        let mut terminal = self
            .wait_research_child_until_terminal(
                ResearchChildWaitTarget {
                    session_id,
                    turn_id,
                    artifact_item_id,
                    child_session_id: spawn_result.child_session_id,
                    agent_path: spawn_result.agent_path.as_str(),
                },
                ResearchChildWaitBuffers {
                    after_sequence: &mut after_sequence,
                    notes: &mut notes,
                },
            )
            .await?;
        self.merge_child_research_output(
            spawn_result.child_session_id,
            &mut notes,
            &mut structured_evidence_messages,
        )
        .await;

        for _ in 0..RESEARCH_SUBAGENT_CONTINUATION_ATTEMPTS {
            if terminal.completed {
                break;
            }
            if terminal.status == "timed_out" {
                let _ = Arc::clone(&self)
                    .close_agent(devo_protocol::CloseAgentParams {
                        session_id,
                        target: spawn_result.child_session_id.to_string(),
                    })
                    .await;
                break;
            }
            let continuation = delegated_research_continuation_message(
                &task,
                terminal.status.as_str(),
                terminal.detail.as_deref(),
                &notes,
            );
            match Arc::clone(&self)
                .send_message(devo_protocol::AgentMessageParams {
                    session_id,
                    target: spawn_result.child_session_id.to_string(),
                    message: continuation,
                })
                .await
            {
                Ok(_) => {
                    terminal = self
                        .wait_research_child_until_terminal(
                            ResearchChildWaitTarget {
                                session_id,
                                turn_id,
                                artifact_item_id,
                                child_session_id: spawn_result.child_session_id,
                                agent_path: spawn_result.agent_path.as_str(),
                            },
                            ResearchChildWaitBuffers {
                                after_sequence: &mut after_sequence,
                                notes: &mut notes,
                            },
                        )
                        .await?;
                    self.merge_child_research_output(
                        spawn_result.child_session_id,
                        &mut notes,
                        &mut structured_evidence_messages,
                    )
                    .await;
                }
                Err(error) => {
                    terminal.detail = Some(format!(
                        "{}; failed to continue child agent: {error}",
                        terminal
                            .detail
                            .as_deref()
                            .unwrap_or("research subagent ended")
                    ));
                    break;
                }
            }
        }

        for _ in 0..RESEARCH_SUBAGENT_RESTART_ATTEMPTS {
            if terminal.completed || !notes.trim().is_empty() {
                break;
            }
            let restart_result = Arc::clone(&self)
                .spawn_agent(devo_protocol::SpawnAgentParams {
                    session_id,
                    message: delegated_research_restart_message(
                        research_brief,
                        research_context,
                        task_index,
                        total_tasks,
                        &task,
                        terminal.status.as_str(),
                        terminal.detail.as_deref(),
                    ),
                    fork_turns: Some("none".to_string()),
                    max_turns: None,
                    tool_policy: devo_protocol::AgentToolPolicy::DeepResearch,
                    context_mode: devo_protocol::AgentContextMode::DeepResearch,
                    ephemeral: false,
                })
                .await?;
            self.remember_research_child_agent(session_id, restart_result.child_session_id)
                .await;
            child_session_ids.push(restart_result.child_session_id);
            terminal_agent_path = restart_result.agent_path.clone();
            let mut restart_after_sequence = 0_u64;
            terminal = self
                .wait_research_child_until_terminal(
                    ResearchChildWaitTarget {
                        session_id,
                        turn_id,
                        artifact_item_id,
                        child_session_id: restart_result.child_session_id,
                        agent_path: restart_result.agent_path.as_str(),
                    },
                    ResearchChildWaitBuffers {
                        after_sequence: &mut restart_after_sequence,
                        notes: &mut notes,
                    },
                )
                .await?;
            self.merge_child_research_output(
                restart_result.child_session_id,
                &mut notes,
                &mut structured_evidence_messages,
            )
            .await;
        }

        if !terminal.completed {
            append_degraded_research_note(
                &mut notes,
                terminal_agent_path.as_str(),
                terminal.status.as_str(),
                terminal.detail.as_deref(),
            );
        }

        if notes.trim().is_empty() {
            notes = format!(
                "Research worker {} did not produce reliable notes for this task. Treat this finding as unavailable and avoid unsupported claims.",
                terminal_agent_path
            );
        }

        let completed_artifact = ResearchArtifactItem {
            artifact_type: ResearchArtifactType::Finding,
            title: artifact_title,
            content: notes.clone(),
        };
        self.complete_item(
            session_id,
            turn_id,
            artifact_item_id,
            artifact_item_seq,
            ItemKind::ResearchArtifact,
            TurnItem::ResearchArtifact(completed_artifact.clone()),
            serde_json::to_value(completed_artifact)
                .expect("serialize completed research artifact"),
        )
        .await;
        for child_session_id in child_session_ids {
            self.forget_research_child_agent(session_id, child_session_id)
                .await;
        }

        Ok(ResearcherOutput {
            title: task.title,
            topic: task.research_topic,
            notes,
            structured_evidence_messages,
            oversized_fetches: Vec::new(),
        })
    }

    async fn wait_research_child_until_terminal(
        self: &Arc<Self>,
        target: ResearchChildWaitTarget<'_>,
        buffers: ResearchChildWaitBuffers<'_>,
    ) -> anyhow::Result<ResearchSubagentTerminal> {
        let ResearchChildWaitTarget {
            session_id,
            turn_id,
            artifact_item_id,
            child_session_id,
            agent_path,
        } = target;
        let ResearchChildWaitBuffers {
            after_sequence,
            notes,
        } = buffers;
        loop {
            let wait_result = Arc::clone(self)
                .wait_agent(devo_protocol::WaitAgentParams {
                    session_id,
                    target: Some(child_session_id.to_string()),
                    after_sequence: Some(*after_sequence),
                    timeout_ms: Some(900_000),
                })
                .await?;
            if wait_result.timed_out && wait_result.events.is_empty() {
                return Ok(ResearchSubagentTerminal {
                    status: "timed_out".to_string(),
                    detail: Some(format!(
                        "research subagent {agent_path} timed out before completing"
                    )),
                    completed: false,
                });
            }
            for event in wait_result.events {
                *after_sequence = (*after_sequence).max(event.sequence);
                match event.kind.as_str() {
                    "assistant_delta" => {
                        if let Some(text) = event.text {
                            if text.is_empty() {
                                continue;
                            }
                            notes.push_str(&text);
                            self.broadcast_event(ServerEvent::ItemDelta {
                                delta_kind: ItemDeltaKind::ResearchArtifactDelta,
                                payload: ItemDeltaPayload {
                                    context: EventContext {
                                        session_id,
                                        turn_id: Some(turn_id),
                                        item_id: Some(artifact_item_id),
                                        seq: 0,
                                    },
                                    delta: text,
                                    stream_index: None,
                                    channel: None,
                                },
                            })
                            .await;
                        }
                    }
                    "status" => match event.status.as_deref() {
                        Some("completed") => {
                            return Ok(ResearchSubagentTerminal {
                                status: "completed".to_string(),
                                detail: event.text,
                                completed: true,
                            });
                        }
                        Some("failed" | "interrupted" | "canceled" | "closed") => {
                            let status = event.status.unwrap_or_else(|| "unknown".to_string());
                            return Ok(ResearchSubagentTerminal {
                                status,
                                detail: event.text,
                                completed: false,
                            });
                        }
                        Some(_) | None => {}
                    },
                    _ => {}
                }
            }
        }
    }

    async fn merge_child_research_output(
        &self,
        child_session_id: SessionId,
        notes: &mut String,
        structured_evidence_messages: &mut Vec<devo_protocol::RequestMessage>,
    ) {
        match self.child_research_output(child_session_id).await {
            Ok((fallback_notes, evidence_messages)) => {
                if notes.trim().is_empty() && !fallback_notes.trim().is_empty() {
                    *notes = fallback_notes;
                }
                structured_evidence_messages.extend(evidence_messages);
            }
            Err(error) => {
                tracing::warn!(
                    child_session_id = %child_session_id,
                    error = %error,
                    "failed to collect child research output"
                );
            }
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

    async fn forget_research_child_agent(
        &self,
        parent_session_id: SessionId,
        child_session_id: SessionId,
    ) {
        let mut research_child_agents = self.research_child_agents.lock().await;
        if let Some(children) = research_child_agents.get_mut(&parent_session_id) {
            children.remove(&child_session_id);
            if children.is_empty() {
                research_child_agents.remove(&parent_session_id);
            }
        }
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

    async fn child_research_output(
        &self,
        child_session_id: SessionId,
    ) -> anyhow::Result<(String, Vec<devo_protocol::RequestMessage>)> {
        let Some(session_arc) = self.sessions.lock().await.get(&child_session_id).cloned() else {
            anyhow::bail!("research subagent session does not exist: {child_session_id}");
        };
        let messages = {
            let session = session_arc.lock().await;
            let core_session = session.core_session.lock().await;
            core_session.messages.clone()
        };
        let notes = messages
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
        Ok((notes, structured_tool_evidence_messages(&messages)))
    }

    async fn handle_final_report_query_event(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        capture: &mut ResearchQueryCapture,
        usage_ledger: &ResearchUsageLedgerRef,
        context_window: Option<u64>,
        event: QueryEvent,
    ) {
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
            QueryEvent::Usage {
                input_tokens,
                output_tokens,
                cache_creation_input_tokens,
                cache_read_input_tokens,
            } => {
                let usage_key = format!("final_report_call_{}", capture.usage_invocation_index);
                self.apply_research_usage(
                    session_id,
                    turn_id,
                    usage_ledger,
                    usage_key,
                    ResearchUsageTotals::from_parts(
                        input_tokens,
                        output_tokens,
                        cache_creation_input_tokens,
                        cache_read_input_tokens,
                    ),
                    context_window,
                )
                .await;
                capture.usage_invocation_index += 1;
            }
            QueryEvent::UsageDelta {
                input_tokens,
                output_tokens,
                cache_creation_input_tokens,
                cache_read_input_tokens,
            } => {
                let usage_key = format!("final_report_call_{}", capture.usage_invocation_index);
                self.apply_research_usage(
                    session_id,
                    turn_id,
                    usage_ledger,
                    usage_key,
                    ResearchUsageTotals::from_parts(
                        input_tokens,
                        output_tokens,
                        cache_creation_input_tokens,
                        cache_read_input_tokens,
                    ),
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

    async fn summarize_oversized_fetches(
        &self,
        runtime: &ResearchModelRuntime<'_>,
        topic: &str,
        fetches: Vec<OversizedFetch>,
        max_summary_chars: usize,
        research_context: &ResearchRequestContext,
        usage_prefix: String,
    ) -> anyhow::Result<Vec<String>> {
        let mut summaries = Vec::new();
        for (index, fetch) in fetches.into_iter().enumerate() {
            let prompt = devo_core::research::prompts::summarize_webpage(max_summary_chars);
            let summary = self
                .model_text_artifact(
                    runtime,
                    prompt,
                    research_context.request_messages(vec![
                        devo_core::research::prompts::research_topic_context(topic),
                        devo_core::research::prompts::source_context(
                            &fetch.source_url,
                            &fetch.source_title,
                            &fetch.content,
                        ),
                    ]),
                    format!("{usage_prefix}_webpage_summary_{index}"),
                    ResearchArtifactType::WebpageSummary,
                    format!("Webpage Summary {}", index + 1),
                )
                .await?;
            summaries.push(summary);
        }
        Ok(summaries)
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

    async fn stream_final_report(
        self: &Arc<Self>,
        runtime: &ResearchModelRuntime<'_>,
        prompt: String,
        question: &str,
        messages: Vec<devo_core::Message>,
    ) -> anyhow::Result<String> {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let callback = Arc::new(move |event: QueryEvent| {
            let _ = tx.send(event);
        });
        let mut final_turn_config = runtime.turn_config.clone();
        final_turn_config.web_search = devo_core::ResolvedWebSearchConfig::Disabled;
        final_turn_config.web_fetch = devo_core::ResolvedWebFetchConfig::Disabled;
        let mut scratch = self.scratch_session(runtime.session_id).await?;
        scratch.config.token_budget = final_turn_config.token_budget();
        scratch.session_context = Some(research_session_context(
            &scratch,
            &final_turn_config,
            research_stage_system(prompt),
        ));
        for message in messages {
            scratch.push_message(message);
        }
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
        let mut capture = ResearchQueryCapture::default();
        let query_result = {
            let query_future = query(
                &mut scratch,
                &final_turn_config,
                runtime
                    .runtime_context
                    .provider_for_route(final_turn_config.provider_route.clone()),
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
                            self.handle_final_report_query_event(
                                runtime.session_id,
                                runtime.turn_id,
                                &mut capture,
                                runtime.usage_ledger,
                                Some(final_turn_config.model.context_window as u64),
                                event,
                            )
                            .await;
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
            self.handle_final_report_query_event(
                runtime.session_id,
                runtime.turn_id,
                &mut capture,
                runtime.usage_ledger,
                Some(final_turn_config.model.context_window as u64),
                event,
            )
            .await;
        }
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

    async fn model_text(
        &self,
        runtime: &ResearchModelRuntime<'_>,
        prompt: String,
        messages: Vec<devo_protocol::RequestMessage>,
        usage_key: String,
    ) -> anyhow::Result<String> {
        self.model_text_with_display(
            runtime,
            prompt,
            messages,
            usage_key,
            ResearchModelTextDisplay::Hidden,
        )
        .await
    }

    async fn model_text_artifact(
        &self,
        runtime: &ResearchModelRuntime<'_>,
        prompt: String,
        messages: Vec<devo_protocol::RequestMessage>,
        usage_key: String,
        artifact_type: ResearchArtifactType,
        title: impl Into<String>,
    ) -> anyhow::Result<String> {
        self.model_text_with_display(
            runtime,
            prompt,
            messages,
            usage_key,
            ResearchModelTextDisplay::ResearchArtifact(StreamedResearchArtifact {
                artifact_type,
                title: title.into(),
            }),
        )
        .await
    }

    async fn model_text_with_display(
        &self,
        runtime: &ResearchModelRuntime<'_>,
        prompt: String,
        messages: Vec<devo_protocol::RequestMessage>,
        usage_key: String,
        display: ResearchModelTextDisplay,
    ) -> anyhow::Result<String> {
        let request = model_text_request(runtime.turn_config, prompt, messages);
        let mut stream = runtime
            .runtime_context
            .provider_for_route(runtime.turn_config.provider_route.clone())
            .completion_stream(request)
            .await?;
        let mut text = String::new();
        let mut reasoning = StreamedTextItem::default();
        let mut artifact = StreamedTextItem::default();
        let mut saw_reasoning = false;
        let mut final_response = None;
        while let Some(event) = stream.next().await {
            match event? {
                StreamEvent::TextStart { .. } | StreamEvent::ReasoningStart { .. } => {}
                StreamEvent::TextDelta { text: delta, .. } => {
                    text.push_str(&delta);
                    if let ResearchModelTextDisplay::ResearchArtifact(display) = &display {
                        self.push_research_artifact_delta(
                            runtime.session_id,
                            runtime.turn_id,
                            &mut artifact,
                            display,
                            delta,
                        )
                        .await;
                    }
                }
                StreamEvent::ReasoningDelta { text: delta, .. } => {
                    saw_reasoning = true;
                    self.push_reasoning_delta(
                        runtime.session_id,
                        runtime.turn_id,
                        &mut reasoning,
                        delta,
                    )
                    .await;
                }
                StreamEvent::ReasoningDone { .. } => {
                    self.complete_reasoning_item(
                        runtime.session_id,
                        runtime.turn_id,
                        &mut reasoning,
                    )
                    .await;
                }
                StreamEvent::MessageDone { response } => {
                    final_response = Some(response);
                }
                StreamEvent::UsageDelta(usage) => {
                    self.apply_research_usage(
                        runtime.session_id,
                        runtime.turn_id,
                        runtime.usage_ledger,
                        usage_key.clone(),
                        ResearchUsageTotals::from_usage(&usage),
                        Some(runtime.turn_config.model.context_window as u64),
                    )
                    .await;
                }
                StreamEvent::ToolCallStart { .. }
                | StreamEvent::HostedToolCallStart { .. }
                | StreamEvent::HostedToolCallDone { .. }
                | StreamEvent::ToolCallInputDelta { .. } => {}
            }
        }
        if let Some(response) = &final_response {
            if text.is_empty() {
                text = response_text(&response.content);
            }
            if !saw_reasoning {
                let reasoning_text = response_reasoning_text(response);
                if !reasoning_text.is_empty() {
                    self.push_reasoning_delta(
                        runtime.session_id,
                        runtime.turn_id,
                        &mut reasoning,
                        reasoning_text,
                    )
                    .await;
                }
            }
            self.apply_research_usage(
                runtime.session_id,
                runtime.turn_id,
                runtime.usage_ledger,
                usage_key,
                ResearchUsageTotals::from_usage(&response.usage),
                Some(runtime.turn_config.model.context_window as u64),
            )
            .await;
        }
        self.complete_reasoning_item(runtime.session_id, runtime.turn_id, &mut reasoning)
            .await;
        if let ResearchModelTextDisplay::ResearchArtifact(display) = &display {
            self.complete_research_artifact_item(
                runtime.session_id,
                runtime.turn_id,
                &mut artifact,
                display,
                &text,
            )
            .await;
        }
        Ok(text)
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
        let network_proxy = runtime_context
            .config_store
            .lock()
            .expect("app config store mutex should not be poisoned")
            .effective_config()
            .provider_http
            .proxy_url
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
                network_proxy,
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
        let (total_input_tokens, total_output_tokens, total_cache_read_tokens) = {
            let mut session = session_arc.lock().await;
            session.summary.total_input_tokens = base.input_tokens + aggregate.input_tokens;
            session.summary.total_output_tokens = base.output_tokens + aggregate.output_tokens;
            session.summary.total_cache_creation_tokens =
                base.cache_creation_input_tokens + aggregate.cache_creation_input_tokens;
            session.summary.total_cache_read_tokens =
                base.cache_read_input_tokens + aggregate.cache_read_input_tokens;
            session.summary.last_query_total_tokens =
                aggregate.input_tokens + aggregate.output_tokens;
            (
                session.summary.total_input_tokens,
                session.summary.total_output_tokens,
                session.summary.total_cache_read_tokens,
            )
        };
        self.broadcast_event(ServerEvent::TurnUsageUpdated(TurnUsageUpdatedPayload {
            session_id,
            turn_id,
            usage: aggregate.to_turn_usage(),
            total_input_tokens,
            total_output_tokens,
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
                cache_creation_input_tokens: session.summary.total_cache_creation_tokens,
                cache_read_input_tokens: session.summary.total_cache_read_tokens,
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

fn response_text(content: &[devo_protocol::ResponseContent]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            devo_protocol::ResponseContent::Text(text) => Some(text.as_str()),
            devo_protocol::ResponseContent::ToolUse { .. }
            | devo_protocol::ResponseContent::HostedToolUse { .. }
            | devo_protocol::ResponseContent::ProviderReasoning { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

fn response_reasoning_text(response: &devo_protocol::ModelResponse) -> String {
    response
        .content
        .iter()
        .filter_map(|block| match block {
            devo_protocol::ResponseContent::ProviderReasoning { payload, .. } => {
                payload.get("thinking").and_then(serde_json::Value::as_str)
            }
            devo_protocol::ResponseContent::Text(_)
            | devo_protocol::ResponseContent::ToolUse { .. }
            | devo_protocol::ResponseContent::HostedToolUse { .. } => None,
        })
        .chain(
            response
                .metadata
                .extras
                .iter()
                .filter_map(|extra| match extra {
                    ResponseExtra::ReasoningText { text } => Some(text.as_str()),
                    ResponseExtra::ProviderSpecific { .. } => None,
                }),
        )
        .collect::<String>()
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
        "Original question:\n{}\n\nResearch tasks: {}",
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

fn request_text_message(text: String) -> devo_protocol::RequestMessage {
    devo_protocol::RequestMessage {
        role: "user".to_string(),
        content: vec![devo_protocol::RequestContent::Text { text }],
    }
}

fn delegated_research_task_message(
    research_brief: &str,
    research_context: &ResearchRequestContext,
    task_index: usize,
    total_tasks: usize,
    task: &ResearchTask,
) -> String {
    let mut message = format!(
        "You are a delegated DeepResearch worker for supervisor task {} of {}.\n\n",
        task_index + 1,
        total_tasks
    );
    message.push_str("<original_research_question>\n");
    message.push_str(research_context.question.trim());
    message.push_str("\n</original_research_question>\n\n");
    if let Some(clarification) = &research_context.clarification {
        message.push_str("<clarification_context>\nQuestion: ");
        message.push_str(clarification.question.trim());
        message.push_str("\nAnswer: ");
        message.push_str(clarification.answer.trim());
        message.push_str("\n</clarification_context>\n\n");
    }
    message.push_str("<research_brief>\n");
    message.push_str(research_brief.trim());
    message.push_str("\n</research_brief>\n\n");
    message.push_str("<research_topic>\nTitle: ");
    message.push_str(task.title.trim());
    message.push_str("\nTopic: ");
    message.push_str(task.research_topic.trim());
    if !task.purpose.trim().is_empty() {
        message.push_str("\nPurpose: ");
        message.push_str(task.purpose.trim());
    }
    if !task.source_strategy.trim().is_empty() {
        message.push_str("\nSource strategy: ");
        message.push_str(task.source_strategy.trim());
    }
    if !task.success_criteria.trim().is_empty() {
        message.push_str("\nSuccess criteria: ");
        message.push_str(task.success_criteria.trim());
    }
    message.push_str("\n</research_topic>\n\n");
    message.push_str(
        "Return concise evidence notes for the parent researcher, not a final report. Do not write files, create local report artifacts, or modify the workspace unless this exact delegated task explicitly requires a local file change. Include searches/tool calls performed, key findings, a source table, conflicts or uncertainty, recommended citations, and any local file paths read for evidence. Do not fabricate citations, URLs, source titles, dates, quotes, or source access.",
    );
    message
}

fn delegated_research_restart_message(
    research_brief: &str,
    research_context: &ResearchRequestContext,
    task_index: usize,
    total_tasks: usize,
    task: &ResearchTask,
    previous_status: &str,
    previous_detail: Option<&str>,
) -> String {
    let mut message = delegated_research_task_message(
        research_brief,
        research_context,
        task_index,
        total_tasks,
        task,
    );
    message.push_str("\n\n<previous_worker_failure>\nStatus: ");
    message.push_str(previous_status.trim());
    if let Some(detail) = previous_detail
        .map(str::trim)
        .filter(|detail| !detail.is_empty())
    {
        message.push_str("\nDetail: ");
        message.push_str(detail);
    }
    message.push_str(
        "\n</previous_worker_failure>\n\nA previous worker could not complete this task. Restart from the task brief, avoid repeating the same failure mode, and return evidence notes with explicit limitations if sources/tools are unavailable. Do not write files or create local report artifacts.",
    );
    message
}

fn delegated_research_continuation_message(
    task: &ResearchTask,
    status: &str,
    detail: Option<&str>,
    partial_notes: &str,
) -> String {
    let mut message = String::from(
        "Your previous delegated research turn ended before completing. Continue the same task now. Preserve any useful evidence already gathered, repair the failure, and return concise evidence notes for the parent researcher. Do not start a final report.\n\n",
    );
    message.push_str("<previous_status>\n");
    message.push_str(status.trim());
    if let Some(detail) = detail.map(str::trim).filter(|detail| !detail.is_empty()) {
        message.push_str(": ");
        message.push_str(detail);
    }
    message.push_str("\n</previous_status>\n\n");
    message.push_str("<research_topic>\nTitle: ");
    message.push_str(task.title.trim());
    message.push_str("\nTopic: ");
    message.push_str(task.research_topic.trim());
    message.push_str("\n</research_topic>\n\n");
    if !partial_notes.trim().is_empty() {
        message.push_str("<partial_notes_already_seen_by_parent>\n");
        message.push_str(&truncate_chars(partial_notes.trim(), 4_000));
        message.push_str("\n</partial_notes_already_seen_by_parent>\n\n");
    }
    message.push_str(
        "Continue from the available context. If a source/tool failed, try an alternate source or report the limitation explicitly. Return only evidence notes, source details, conflicts or uncertainty, and recommended citations. Do not write files or create local report artifacts.",
    );
    message
}

fn append_degraded_research_note(
    notes: &mut String,
    agent_path: &str,
    status: &str,
    detail: Option<&str>,
) {
    if !notes.ends_with('\n') && !notes.is_empty() {
        notes.push('\n');
    }
    if !notes.is_empty() {
        notes.push('\n');
    }
    notes.push_str("Research worker status: ");
    notes.push_str(agent_path);
    notes.push_str(" ended with ");
    notes.push_str(status.trim());
    if let Some(detail) = detail.map(str::trim).filter(|detail| !detail.is_empty()) {
        notes.push_str(". Detail: ");
        notes.push_str(detail);
    }
    notes.push_str(
        "\nProceeding with partial evidence for this task. Treat missing citations or unsupported claims as unavailable rather than inferred.",
    );
}

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

fn model_text_request(
    turn_config: &TurnConfig,
    stage_prompt: String,
    messages: Vec<devo_protocol::RequestMessage>,
) -> devo_protocol::ModelRequest {
    let resolved = turn_config
        .model
        .resolve_reasoning_effort_selection(turn_config.reasoning_effort_selection.as_deref());
    devo_protocol::ModelRequest {
        model: turn_config.provider_request_model(&resolved.request_model),
        system: Some(research_stage_system(stage_prompt))
            .filter(|system| !system.trim().is_empty()),
        messages,
        max_tokens: turn_config
            .model
            .max_tokens
            .map_or(turn_config.token_budget().max_output_tokens, |value| {
                value as usize
            }),
        tools: None,
        hosted_tools: Vec::new(),
        sampling: devo_protocol::SamplingControls {
            temperature: turn_config.model.temperature,
            top_p: turn_config.model.top_p,
            top_k: turn_config.model.top_k.map(|value| value as u32),
        },
        request_thinking: resolved.request_thinking,
        reasoning_effort: resolved.request_reasoning_effort,
        extra_body: resolved.extra_body,
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

#[cfg(test)]
fn extract_source_url(input: &serde_json::Value, output: &serde_json::Value) -> Option<String> {
    input
        .get("url")
        .and_then(serde_json::Value::as_str)
        .or_else(|| output.get("url").and_then(serde_json::Value::as_str))
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
fn extract_source_title(
    output: &serde_json::Value,
    display_content: Option<&str>,
) -> Option<String> {
    output
        .get("title")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            output
                .get("metadata")
                .and_then(|metadata| metadata.get("title"))
                .and_then(serde_json::Value::as_str)
        })
        .or(display_content)
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(|title| truncate_chars(title, 240))
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
    fn extract_fetch_metadata_prefers_visible_url_and_title() {
        // Trace: L2-DES-RESEARCH-001
        // Verifies: oversized local web fetch summaries receive source metadata when available.
        let input = json!({"url": "https://example.com/page"});
        let output = json!({"title": "Example Page"});

        assert_eq!(
            extract_source_url(&input, &output),
            Some("https://example.com/page".to_string())
        );
        assert_eq!(
            extract_source_title(&output, None),
            Some("Example Page".to_string())
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
            "Original question:\nWhat changed?\n\nResearch tasks: 2\n\nSource/reference hints:\nhttps://example.com/a\nhttps://example.com/b"
        );
    }
}

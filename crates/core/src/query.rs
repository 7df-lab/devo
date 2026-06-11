use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use devo_protocol::HostedToolDefinition;
use devo_protocol::HostedWebFetchTool;
use devo_protocol::HostedWebSearchTool;
use devo_protocol::ModelRequest;
use devo_protocol::RequestContent;
use devo_protocol::RequestMessage;
use devo_protocol::ResolvedThinkingRequest;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseExtra;
use devo_protocol::SamplingControls;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use futures::StreamExt;
use tokio::time::sleep;
use tracing::debug;
use tracing::info;
use tracing::info_span;
use tracing::warn;

use crate::tools::ToolAgentScope;
use crate::tools::ToolCall;
use crate::tools::ToolContent;
use crate::tools::ToolRegistry;
use crate::tools::ToolRuntime;
use crate::tools::deferred_loading::is_subagent_agent_coordination_tool;
use devo_provider::ModelProviderSDK;

use crate::AgentError;
use crate::ContentBlock;
use crate::Message;
use crate::Model;
use crate::Role;
use crate::SessionState;
use crate::TurnConfig;
use crate::context::AgentsMdDiffFragment;
use crate::context::AgentsMdManager;
use crate::context::ContextualUserFragment;
use crate::context::SessionContext;
use crate::context::TurnContext;
use crate::context::load_workspace_instructions;
use crate::context::turn_aborted::TurnAborted;
use crate::history::ContextView;
use crate::history::History;
use crate::history::TokenInfo;
use crate::history::compaction::CompactAction;
use crate::history::compaction::CompactionConfig;
use crate::history::compaction::CompactionKind;
use crate::history::compaction::compact_history;
use crate::history::summarizer::DefaultHistorySummarizer;
use crate::response_item::ResponseItem;
use crate::response_item::message_to_response_items;

const SUBAGENT_MODE_REMINDER: &str = "<system-reminder>\nYou are running as a sub-agent. Complete the delegated task using the available non-agent tools. Do not call agent coordination tools such as spawn_agent, send_message, wait_agent, list_agents, or close_agent; report progress and final results through assistant output.\n</system-reminder>";

fn hosted_tools_for_web_capabilities(
    web_search: &devo_config::ResolvedWebSearchConfig,
    web_fetch: devo_config::ResolvedWebFetchConfig,
) -> Vec<HostedToolDefinition> {
    let mut hosted_tools = Vec::new();
    if matches!(web_search, devo_config::ResolvedWebSearchConfig::Provider) {
        hosted_tools.push(HostedToolDefinition::WebSearch(HostedWebSearchTool::new()));
    }
    if web_fetch.is_provider() {
        hosted_tools.push(HostedToolDefinition::WebFetch(HostedWebFetchTool::new()));
    }
    hosted_tools
}

#[cfg(test)]
fn hosted_tools_for_web_search(
    web_search: &devo_config::ResolvedWebSearchConfig,
) -> Vec<HostedToolDefinition> {
    hosted_tools_for_web_capabilities(web_search, devo_config::ResolvedWebFetchConfig::Disabled)
}

fn estimate_request_prompt_tokens(request: &ModelRequest) -> usize {
    let system_bytes = request.system.as_ref().map_or(0, String::len);
    let message_bytes = request
        .messages
        .iter()
        .map(|message| serde_json::to_string(message).map_or(0, |json| json.len()))
        .sum::<usize>();
    let tool_bytes = request
        .tools
        .as_ref()
        .map(|tools| serde_json::to_string(tools).map_or(0, |json| json.len()))
        .unwrap_or(0);
    let hosted_tool_bytes =
        serde_json::to_string(&request.hosted_tools).map_or(0, |json| json.len());
    (system_bytes + message_bytes + tool_bytes + hosted_tool_bytes).div_ceil(4)
}

/// Events emitted during a query for the caller (CLI/UI) to observe.
#[derive(Debug, Clone)]
pub enum QueryEvent {
    /// Incremental text from the assistant.
    TextDelta(String),
    /// Incremental reasoning text from the assistant.
    ReasoningDelta(String),
    /// Current reasoning block completed.
    ReasoningCompleted,
    /// Incremental token usage update from the provider stream.
    /// TODO: Review the mechanism from the OpenAI API / Anthropic API documentation.
    UsageDelta {
        input_tokens: usize,
        output_tokens: usize,
        cache_creation_input_tokens: Option<usize>,
        cache_read_input_tokens: Option<usize>,
    },
    /// The assistant started a tool call.
    ToolUseStart {
        /// Stable provider-issued tool use identifier.
        id: String,
        /// Tool name selected by the model.
        name: String,
        /// Fully decoded tool input payload, when available.
        input: serde_json::Value,
    },
    /// Incremental output delta from a running tool.
    ToolProgress {
        tool_use_id: String,
        content: String,
    },
    /// A tool call completed.
    ToolResult {
        tool_use_id: String,
        tool_name: String,
        input: serde_json::Value,
        content: ToolContent,
        display_content: Option<String>,
        is_error: bool,
        /// Human-readable summary for client-side rendering (e.g. "bash: npm run dev").
        summary: String,
    },
    /// A turn is complete (model stopped generating).
    TurnComplete { stop_reason: StopReason },
    /// Token usage update.
    Usage {
        input_tokens: usize,
        output_tokens: usize,
        cache_creation_input_tokens: Option<usize>,
        cache_read_input_tokens: Option<usize>,
    },
}

/// Callback for streaming query events to the UI layer.
pub type EventCallback = Arc<dyn Fn(QueryEvent) + Send + Sync>;

// ---------------------------------------------------------------------------
// Error classification (capability 3.2)
// ---------------------------------------------------------------------------

enum ErrorClass {
    ContextTooLong,
    ParameterError,
    FileContentAnomaly,
    AuthenticationFailure,
    FeatureUnavailable,
    TaskNotFound,
    RateLimit,
    NoApiPermission,
    FileTooLarge,
    ServerError,
    Unretryable,
}

enum ProviderRetryDecision {
    RetryAfter(Duration),
    CompactAndRetry,
    Fail,
}

fn classify_error(e: &anyhow::Error) -> ErrorClass {
    let msg = e.to_string().to_lowercase();
    // TODO: Expand the error of ContextTooLong
    if msg.contains("context_too_long") {
        ErrorClass::ContextTooLong
    } else if msg.contains("401")
        || msg.contains("authentication failure")
        || msg.contains("token timeout")
        || msg.contains("unauthorized")
        || msg.contains("api key")
    {
        ErrorClass::AuthenticationFailure
    } else if msg.contains("404")
        && (msg.contains("feature not available")
            || msg.contains("fine-tuning feature not available"))
    {
        ErrorClass::FeatureUnavailable
    } else if msg.contains("404")
        && (msg.contains("task does not exist")
            || msg.contains("does not exist")
            || msg.contains("not found"))
    {
        ErrorClass::TaskNotFound
    } else if msg.contains("429") || msg.contains("rate limit") {
        ErrorClass::RateLimit
    } else if msg.contains("434") || msg.contains("no api permission") || msg.contains("beta phase")
    {
        ErrorClass::NoApiPermission
    } else if msg.contains("435")
        || msg.contains("file size exceeds 100mb")
        || msg.contains("smaller than 100mb")
    {
        ErrorClass::FileTooLarge
    } else if msg.contains("400")
        && (msg.contains("file content anomaly")
            || msg.contains("jsonl file content")
            || msg.contains("jsonl"))
    {
        ErrorClass::FileContentAnomaly
    } else if msg.contains("400")
        || msg.contains("parameter error")
        || msg.contains("invalid parameter")
        || msg.contains("bad request")
    {
        ErrorClass::ParameterError
    } else if msg.starts_with('5')
        || msg.contains("500")
        || msg.contains("502")
        || msg.contains("503")
        || msg.contains("504")
        || msg.contains("internal server error")
        || msg.contains("server error occurred while processing the request")
    {
        ErrorClass::ServerError
    } else {
        ErrorClass::Unretryable
    }
}

fn provider_retry_decision(
    error: &anyhow::Error,
    retry_count: &mut usize,
    context_compacted: &mut bool,
) -> ProviderRetryDecision {
    match classify_error(error) {
        ErrorClass::ContextTooLong => {
            if *context_compacted {
                ProviderRetryDecision::Fail
            } else {
                *context_compacted = true;
                ProviderRetryDecision::CompactAndRetry
            }
        }
        ErrorClass::RateLimit | ErrorClass::ServerError => {
            if *retry_count >= MAX_RETRIES {
                ProviderRetryDecision::Fail
            } else {
                *retry_count += 1;
                ProviderRetryDecision::RetryAfter(retry_backoff_duration(*retry_count))
            }
        }
        ErrorClass::ParameterError
        | ErrorClass::FileContentAnomaly
        | ErrorClass::AuthenticationFailure
        | ErrorClass::FeatureUnavailable
        | ErrorClass::TaskNotFound
        | ErrorClass::NoApiPermission
        | ErrorClass::FileTooLarge
        | ErrorClass::Unretryable => ProviderRetryDecision::Fail,
    }
}

// ---------------------------------------------------------------------------
// Session compaction
// ---------------------------------------------------------------------------

/// Compact session messages using LLM-backed summarization.
///
/// Converts session messages to ResponseItems, runs compact_history()
/// with the history module's LLM summarizer, and converts the compacted
/// items back to Messages.
async fn summarize_and_compact(
    session: &mut SessionState,
    provider: &Arc<dyn ModelProviderSDK>,
    request_model: &str,
    max_tokens: usize,
) {
    let items: Vec<ResponseItem> = session
        .prompt_source_messages()
        .iter()
        .cloned()
        .flat_map(message_to_response_items)
        .collect();

    let token_info = TokenInfo {
        input_tokens: session.total_input_tokens,
        cached_input_tokens: session.total_cache_read_tokens,
        output_tokens: session.total_output_tokens,
    };

    let config = CompactionConfig {
        budget: session.config.token_budget.clone(),
        kind: CompactionKind::Proactive,
    };

    let summarizer =
        DefaultHistorySummarizer::with_slug(Arc::clone(provider), request_model, max_tokens);

    match compact_history(&items, &token_info, &summarizer, &config).await {
        Ok(CompactAction::Replaced(compacted_items)) => {
            let new_messages: Vec<Message> = compacted_items
                .into_iter()
                .filter_map(|item| match item {
                    ResponseItem::Message(msg) => Some(msg),
                    _ => None,
                })
                .collect();
            let removed = session
                .prompt_source_messages()
                .len()
                .saturating_sub(new_messages.len());
            info!("LLM compaction removed {removed} messages");
            session.set_prompt_messages(new_messages);
        }
        Ok(CompactAction::Skipped) => {
            debug!("LLM compaction skipped, nothing to compact");
        }
        Err(e) => {
            warn!("LLM compaction failed: {e}");
        }
    }
}

// ---------------------------------------------------------------------------
// Micro compact
// ---------------------------------------------------------------------------

/// TODO: Now, the micro compact acts like a truncation, however, we already
/// have truncation policy, should follow model's truncation policy, so the
/// micro compact should be removed in the future.
const MICRO_COMPACT_THRESHOLD: usize = 10_000;

fn micro_compact(content: String) -> String {
    if content.len() > MICRO_COMPACT_THRESHOLD {
        let truncate_at = content
            .char_indices()
            .map(|(index, _)| index)
            .take_while(|index| *index <= MICRO_COMPACT_THRESHOLD)
            .last()
            .unwrap_or(0);
        let mut truncated = content[..truncate_at].to_string();
        truncated.push_str("\n...[truncated]");
        truncated
    } else {
        content
    }
}

fn preserve_full_tool_result(tool_name: Option<&str>) -> bool {
    matches!(tool_name, Some("wait_agent" | "subagent_result"))
}

fn insert_subagent_request_reminders(messages: &mut Vec<RequestMessage>) {
    let insert_at = messages
        .iter()
        .rposition(is_user_text_message)
        .unwrap_or(messages.len());
    messages.splice(
        insert_at..insert_at,
        [request_text_message(SUBAGENT_MODE_REMINDER.to_string())],
    );
}

fn insert_goal_context_message(messages: &mut Vec<RequestMessage>, goal_context: &str) {
    let insert_at = if messages.last().is_some_and(is_visible_user_text_message) {
        messages.len().saturating_sub(1)
    } else {
        messages.len()
    };
    messages.splice(
        insert_at..insert_at,
        [request_text_message(goal_context.to_string())],
    );
}

fn request_text_message(text: String) -> RequestMessage {
    RequestMessage {
        role: Role::User.as_str().to_string(),
        content: vec![RequestContent::Text { text }],
    }
}

fn is_user_text_message(message: &RequestMessage) -> bool {
    message.role == Role::User.as_str()
        && message
            .content
            .iter()
            .any(|content| matches!(content, RequestContent::Text { .. }))
}

fn is_visible_user_text_message(message: &RequestMessage) -> bool {
    is_user_text_message(message) && !is_injected_context_message(message)
}

fn is_injected_context_message(message: &RequestMessage) -> bool {
    message.role == Role::User.as_str()
        && message.content.iter().any(|content| match content {
            RequestContent::Text { text } => {
                let trimmed = text.trim_start();
                trimmed.starts_with("<environment_context>")
                    || trimmed.starts_with("<context_changes>")
                    || trimmed.starts_with("<user_instructions_updates>")
                    || trimmed.starts_with("<user_instructions>")
            }
            RequestContent::Reasoning { .. }
            | RequestContent::ToolUse { .. }
            | RequestContent::ToolResult { .. } => false,
        })
}

fn compact_tool_content(content: ToolContent) -> ToolContent {
    match content {
        ToolContent::Text(text) => ToolContent::Text(micro_compact(text)),
        ToolContent::Json(json) => ToolContent::Json(json),
        ToolContent::Mixed { text, json } => ToolContent::Mixed {
            text: text.map(micro_compact),
            json,
        },
    }
}

fn tool_content_model_bytes(content: &ToolContent) -> usize {
    match content {
        ToolContent::Text(text) => text.len(),
        ToolContent::Json(json) => json.to_string().len(),
        ToolContent::Mixed { text, json } => {
            text.as_ref().map_or(0, String::len)
                + json.as_ref().map_or(0, |json| json.to_string().len())
        }
    }
}

fn normalize_hosted_tool_id(index: usize, id: String, name: &str) -> String {
    if id.is_empty() {
        format!("hosted_{}_{index}", name.replace('-', "_"))
    } else {
        id
    }
}

fn normalize_hosted_tool_name(name: String) -> String {
    if name.is_empty() {
        "web_search".to_string()
    } else {
        name
    }
}

fn hosted_tool_input_or_previous(
    input: serde_json::Value,
    previous: Option<&serde_json::Value>,
) -> serde_json::Value {
    if matches!(&input, serde_json::Value::Object(map) if map.is_empty()) {
        previous.cloned().unwrap_or(input)
    } else {
        input
    }
}

fn emit_hosted_tool_start<F>(
    emit: &F,
    emitted_tool_use_starts: &mut HashSet<String>,
    id: &str,
    name: &str,
    input: &serde_json::Value,
) where
    F: Fn(QueryEvent),
{
    if emitted_tool_use_starts.insert(id.to_string()) {
        emit(QueryEvent::ToolUseStart {
            id: id.to_string(),
            name: name.to_string(),
            input: input.clone(),
        });
    }
}

struct HostedToolResultEvent<'a> {
    id: &'a str,
    name: &'a str,
    input: &'a serde_json::Value,
    output: Option<serde_json::Value>,
    status: Option<String>,
}

fn emit_hosted_tool_result<F>(
    emit: &F,
    emitted_tool_results: &mut HashSet<String>,
    session_cwd: &std::path::Path,
    event: HostedToolResultEvent<'_>,
) where
    F: Fn(QueryEvent),
{
    let HostedToolResultEvent {
        id,
        name,
        input,
        output,
        status,
    } = event;
    if !emitted_tool_results.insert(id.to_string()) {
        return;
    }

    let text = hosted_tool_result_text(name, input, output.as_ref(), status.as_deref());
    let summary = crate::tools::tool_summary::tool_summary(name, input, session_cwd);
    emit(QueryEvent::ToolResult {
        tool_use_id: id.to_string(),
        tool_name: name.to_string(),
        input: input.clone(),
        content: ToolContent::Text(text.clone()),
        display_content: Some(micro_compact(text)),
        is_error: hosted_tool_status_is_error(status.as_deref()),
        summary,
    });
}

fn hosted_tool_result_text(
    _name: &str,
    _input: &serde_json::Value,
    _output: Option<&serde_json::Value>,
    status: Option<&str>,
) -> String {
    let status = status
        .filter(|status| !status.is_empty())
        .unwrap_or("completed");
    format!("status: {status}")
}
fn hosted_tool_status_is_error(status: Option<&str>) -> bool {
    status
        .map(str::to_ascii_lowercase)
        .is_some_and(|status| matches!(status.as_str(), "error" | "errored" | "failed"))
}

// ---------------------------------------------------------------------------
// Main query loop
// ---------------------------------------------------------------------------

const MAX_RETRIES: usize = 5;
const INITIAL_RETRY_BACKOFF_MS: u64 = 250;

/// TODO: The body of `query` is too lengthy, we should move out `stream lop` out, I am
/// not sure whether we should do this.
/// The recursive agent loop the beating heart of the runtime.
///
/// The implementation refers to Claude Code's `query.ts`. It drives
/// multi-turn conversations by:
///
/// 1. Building the model request from session state
/// 2. Streaming the model response
/// 3. Collecting assistant text and tool_use blocks
/// 4. Executing tool calls via the orchestrator
/// 5. Appending tool_result messages
/// 6. Recursing if the model wants to continue
///
/// The loop terminates when:
/// - The model emits `end_turn` with no tool calls
/// - An unrecoverable error occurs
pub async fn query(
    session: &mut SessionState,
    turn_config: &TurnConfig,
    provider: Arc<dyn ModelProviderSDK>,
    registry: Arc<ToolRegistry>,
    runtime: &ToolRuntime,
    on_event: Option<EventCallback>,
) -> Result<(), AgentError> {
    // emit is the event callback function.
    let emit = |event: QueryEvent| {
        if let Some(ref cb) = on_event {
            cb(event);
        }
    };

    let agents_md_manager = AgentsMdManager::new(session.config.agents_md.clone());
    let current_agents_snapshot = load_workspace_instructions(&session.cwd, &agents_md_manager);
    let agent_scope = runtime.agent_scope();
    let mut request_tools = registry.tool_definitions();
    if agent_scope == ToolAgentScope::Subagent {
        request_tools.retain(|tool| !is_subagent_agent_coordination_tool(&tool.name));
    }
    if !turn_config.web_search.is_local() {
        request_tools.retain(|tool| tool.name != "web_search");
    }
    if !turn_config.web_fetch.is_local() {
        request_tools.retain(|tool| tool.name != "webfetch");
    }

    if session.session_context.is_none() {
        session.session_context = Some(SessionContext::capture(
            &turn_config.model,
            turn_config.thinking_selection.as_deref(),
            &session.cwd,
            current_agents_snapshot.clone(),
        ));
    }
    let current_turn_context =
        TurnContext::capture(session, turn_config, current_agents_snapshot.clone());
    let context_changes =
        current_turn_context.context_changes_since(session.latest_turn_context.as_ref());
    session.insert_context_message(context_changes.to_message());
    if let Some(previous_turn_context) = session.latest_turn_context.as_ref()
        && let Some(diff) = AgentsMdManager::diff(
            previous_turn_context.observed_agents_snapshot.as_ref(),
            current_agents_snapshot.as_ref(),
        )
    {
        session.insert_context_message(AgentsMdDiffFragment::new(diff).to_message());
    }
    session.latest_turn_context = Some(current_turn_context.clone());
    let session_context = session
        .session_context
        .clone()
        .expect("session context should be initialized");
    let prefetched_user_inputs = session_context.prefix_user_inputs();

    let mut retry_count: usize = 0;
    let mut context_compacted = false;
    let mut budget_steer_injected = false;

    if session.turn_state.is_none() {
        session.start_turn(devo_protocol::TurnKind::Regular);
    }

    'query_loop: loop {
        let pending = session.take_turn_pending_input();

        // If the user interrupted the assistant mid-turn, explain the interruption
        if !pending.is_empty()
            && session
                .messages
                .last()
                .is_some_and(|m| m.role == Role::Assistant)
        {
            let fragment = TurnAborted::new(TurnAborted::INTERRUPTED_GUIDANCE);
            if let ResponseItem::Message(msg) = fragment.to_response_item() {
                session.push_message(msg);
            }
        }

        for item in &pending {
            match &item.kind {
                devo_protocol::PendingInputKind::UserText { text } => {
                    session.push_message(Message::user(text.clone()));
                }
                devo_protocol::PendingInputKind::UserInput { prompt_text, .. } => {
                    session.push_message(Message::user(prompt_text.clone()));
                }
                devo_protocol::PendingInputKind::ToolCallBlockedByHook {
                    tool_use_id,
                    reason,
                } => {
                    session.push_message(Message::user(format!(
                        "[Tool call {} was blocked: {}]",
                        tool_use_id, reason
                    )));
                }
                devo_protocol::PendingInputKind::BudgetLimitSteering => {
                    session.push_message(Message::system(
                        "Note: The conversation is approaching the token budget limit. \
                         Please be concise and consider wrapping up the current task.",
                    ));
                }
            }
        }

        // 1.3 + 1.7: Check token budget and compact before building the request
        if session.last_input_tokens > 0
            && session
                .config
                .token_budget
                .should_compact(session.last_input_tokens)
        {
            if !budget_steer_injected {
                if let Some(turn) = session.turn_state.as_mut() {
                    turn.push_pending_input(devo_protocol::PendingInputItem {
                        kind: devo_protocol::PendingInputKind::BudgetLimitSteering,
                        metadata: None,
                        created_at: chrono::Utc::now(),
                    });
                }
                budget_steer_injected = true;
            }
            info!("token budget threshold exceeded, running LLM compaction");
            summarize_and_compact(
                session,
                &provider,
                &turn_config.request_model,
                turn_config.model.max_tokens.unwrap_or(4096) as usize,
            )
            .await;
        }

        session.turn_count += 1;
        let turn_span = info_span!(
            "turn",
            turn = session.turn_count,
            session_id = %session.id,
            model = %turn_config.model.slug,
            cwd = %session.cwd.display()
        );
        let _turn_guard = turn_span.enter();
        info!("starting turn");

        // Build model request from the session-locked prefix.
        let request_system = {
            let mut system = session_context.build_system_prompt();
            if !matches!(
                &turn_config.web_search,
                devo_config::ResolvedWebSearchConfig::Disabled
            ) {
                if !system.trim().is_empty() {
                    system.push_str("\n\n");
                }
                system.push_str(&crate::tools::websearch_prompt::web_search_prompt());
            }
            Some(system).filter(|system| !system.trim().is_empty())
        };

        // resolve thinking request parameter
        let ResolvedThinkingRequest {
            request_model,
            request_thinking,
            request_reasoning_effort,
            extra_body,
            effective_reasoning_effort: _,
        } = turn_config
            .model
            .resolve_thinking_selection(turn_config.thinking_selection.as_deref());
        let provider_request_model = turn_config.provider_request_model(&request_model);

        let prompt_source_message_count = session.prompt_source_messages().len();
        let history_items = session
            .prompt_source_messages()
            .iter()
            .cloned()
            .flat_map(message_to_response_items)
            .collect::<Vec<_>>();
        let prompt_source_item_count = history_items.len();
        let history = History {
            items: history_items,
            token_info: TokenInfo::default(),
            context: ContextView::new(
                std::env::consts::OS,
                session_context.environment.shell.clone(),
                session_context.environment.timezone.clone(),
                session_context.model.slug.clone(),
                session_context
                    .reasoning_effort
                    .map(|effort| effort.label().to_lowercase()),
                Some(session_context.persona.as_str().to_string()),
                session_context.environment.current_date.clone(),
                session_context.environment.cwd.display().to_string(),
            ),
        };
        let mut messages = history
            .for_prompt_with_prefix(&prefetched_user_inputs, &turn_config.model.input_modalities);
        if let Some(goal_context) = session.goal_context_prompt() {
            insert_goal_context_message(&mut messages, &goal_context);
        }
        if agent_scope == ToolAgentScope::Subagent {
            insert_subagent_request_reminders(&mut messages);
        }

        let request = ModelRequest {
            model: provider_request_model,
            system: request_system,
            messages,
            max_tokens: turn_config
                .model
                .max_tokens
                .map_or(session.config.token_budget.max_output_tokens, |value| {
                    value as usize
                }),
            tools: Some(request_tools.clone()),
            hosted_tools: hosted_tools_for_web_capabilities(
                &turn_config.web_search,
                turn_config.web_fetch,
            ),
            sampling: SamplingControls {
                temperature: turn_config.model.temperature,
                top_p: turn_config.model.top_p,
                top_k: turn_config.model.top_k.map(|value| value as u32),
            },
            thinking: request_thinking,
            reasoning_effort: request_reasoning_effort,
            extra_body,
        };
        session.prompt_token_estimate = estimate_request_prompt_tokens(&request);
        debug!(
            prompt_source_messages = prompt_source_message_count,
            prompt_source_items = prompt_source_item_count,
            prefix_user_inputs = prefetched_user_inputs.len(),
            request_messages = request.messages.len(),
            exposed_tools = request.tools.as_ref().map_or(0, Vec::len),
            prompt_token_estimate = session.prompt_token_estimate,
            max_tokens = request.max_tokens,
            has_system = request.system.is_some(),
            "built model request"
        );

        // Stream with error classification
        let stream_result = provider.completion_stream(request).await;

        let mut stream = match stream_result {
            Ok(s) => s,
            Err(e) => {
                warn!(
                    provider = provider.name(),
                    model = %turn_config.model.slug,
                    turn = session.turn_count,
                    error = ?e,
                    "failed to create provider stream"
                );
                match provider_retry_decision(&e, &mut retry_count, &mut context_compacted) {
                    ProviderRetryDecision::CompactAndRetry => {
                        warn!("context_too_long - compacting and retrying");
                        summarize_and_compact(
                            session,
                            &provider,
                            &turn_config.request_model,
                            turn_config.model.max_tokens.unwrap_or(4096) as usize,
                        )
                        .await;
                        session.turn_count -= 1;
                        continue;
                    }
                    ProviderRetryDecision::RetryAfter(backoff) => {
                        warn!(
                            attempt = retry_count,
                            backoff_ms = backoff.as_millis(),
                            "transient provider error - retrying with exponential backoff"
                        );
                        sleep(backoff).await;
                        session.turn_count -= 1;
                        continue;
                    }
                    ProviderRetryDecision::Fail => {
                        return Err(AgentError::Provider(e));
                    }
                }
            }
        };

        // HTTP return ok, then processing Server Sent Event

        let mut assistant_text = String::new();
        let mut reasoning_text = String::new();
        let mut tool_uses: Vec<(usize, String, String, serde_json::Value, String, bool)> =
            Vec::new();
        let mut hosted_tool_inputs: HashMap<String, (usize, String, serde_json::Value)> =
            HashMap::new();
        let mut emitted_tool_use_starts: HashSet<String> = HashSet::new();
        let mut emitted_hosted_tool_starts: HashSet<String> = HashSet::new();
        let mut emitted_hosted_tool_results: HashSet<String> = HashSet::new();
        let mut final_response = None;
        let mut stop_reason = None;

        while let Some(event) = stream.next().await {
            match event {
                Ok(StreamEvent::TextStart { .. }) => {}
                Ok(StreamEvent::TextDelta { text, .. }) => {
                    assistant_text.push_str(&text);
                    emit(QueryEvent::TextDelta(text));
                }
                Ok(StreamEvent::ReasoningStart { .. }) => {}
                Ok(StreamEvent::ReasoningDelta { text, .. }) => {
                    reasoning_text.push_str(&text);
                    emit(QueryEvent::ReasoningDelta(text));
                }
                Ok(StreamEvent::ReasoningDone { .. }) => {
                    emit(QueryEvent::ReasoningCompleted);
                }
                Ok(StreamEvent::ToolCallStart {
                    index,
                    id,
                    name,
                    input,
                }) => {
                    tool_uses.push((index, id, name, input, String::new(), false));
                }
                Ok(StreamEvent::HostedToolCallStart {
                    index,
                    id,
                    name,
                    input,
                }) => {
                    let id = normalize_hosted_tool_id(index, id, &name);
                    let name = normalize_hosted_tool_name(name);
                    hosted_tool_inputs.insert(id.clone(), (index, name.clone(), input.clone()));
                    emit_hosted_tool_start(
                        &emit,
                        &mut emitted_hosted_tool_starts,
                        &id,
                        &name,
                        &input,
                    );
                }
                Ok(StreamEvent::HostedToolCallDone {
                    index,
                    id,
                    name,
                    input,
                    output,
                    status,
                }) => {
                    let id = normalize_hosted_tool_id(index, id, &name);
                    let name = normalize_hosted_tool_name(name);
                    let previous_input = hosted_tool_inputs
                        .get(&id)
                        .map(|(_, _, previous_input)| previous_input);
                    let input = hosted_tool_input_or_previous(input, previous_input);
                    hosted_tool_inputs.insert(id.clone(), (index, name.clone(), input.clone()));
                    emit_hosted_tool_start(
                        &emit,
                        &mut emitted_hosted_tool_starts,
                        &id,
                        &name,
                        &input,
                    );
                    emit_hosted_tool_result(
                        &emit,
                        &mut emitted_hosted_tool_results,
                        &session.cwd,
                        HostedToolResultEvent {
                            id: &id,
                            name: &name,
                            input: &input,
                            output,
                            status,
                        },
                    );
                }
                Ok(StreamEvent::ToolCallInputDelta {
                    index,
                    partial_json,
                }) => {
                    if let Some(tool_use) = tool_uses
                        .iter_mut()
                        .rev()
                        .find(|(tool_index, ..)| *tool_index == index)
                    {
                        tool_use.4.push_str(&partial_json);
                        tool_use.5 = true;
                    }
                }
                Ok(StreamEvent::MessageDone { response }) => {
                    stop_reason = response.stop_reason.clone();
                    final_response = Some(response.clone());

                    // Accumulate all usage counters at completion time.
                    session.total_input_tokens += response.usage.input_tokens;
                    session.total_output_tokens += response.usage.output_tokens;
                    session.total_cache_creation_tokens +=
                        response.usage.cache_creation_input_tokens.unwrap_or(0);
                    session.total_cache_read_tokens +=
                        response.usage.cache_read_input_tokens.unwrap_or(0);
                    session.last_input_tokens = response.usage.input_tokens;

                    emit(QueryEvent::Usage {
                        input_tokens: response.usage.input_tokens,
                        output_tokens: response.usage.output_tokens,
                        cache_creation_input_tokens: response.usage.cache_creation_input_tokens,
                        cache_read_input_tokens: response.usage.cache_read_input_tokens,
                    });
                }
                Ok(StreamEvent::UsageDelta(usage)) => {
                    emit(QueryEvent::UsageDelta {
                        input_tokens: usage.input_tokens,
                        output_tokens: usage.output_tokens,
                        cache_creation_input_tokens: usage.cache_creation_input_tokens,
                        cache_read_input_tokens: usage.cache_read_input_tokens,
                    });
                }
                Err(e) => {
                    warn!(
                        provider = provider.name(),
                        model = %turn_config.model.slug,
                        turn = session.turn_count,
                        error = ?e,
                        "stream error"
                    );
                    if !assistant_text.is_empty()
                        || !reasoning_text.is_empty()
                        || !tool_uses.is_empty()
                        || !hosted_tool_inputs.is_empty()
                        || final_response.is_some()
                    {
                        return Err(AgentError::Provider(e));
                    }

                    match provider_retry_decision(&e, &mut retry_count, &mut context_compacted) {
                        ProviderRetryDecision::CompactAndRetry => {
                            warn!("context_too_long - compacting and retrying");
                            summarize_and_compact(
                                session,
                                &provider,
                                &turn_config.request_model,
                                turn_config.model.max_tokens.unwrap_or(4096) as usize,
                            )
                            .await;
                            session.turn_count -= 1;
                            continue 'query_loop;
                        }
                        ProviderRetryDecision::RetryAfter(backoff) => {
                            warn!(
                                attempt = retry_count,
                                backoff_ms = backoff.as_millis(),
                                "transient provider stream error - retrying with exponential backoff"
                            );
                            sleep(backoff).await;
                            session.turn_count -= 1;
                            continue 'query_loop;
                        }
                        ProviderRetryDecision::Fail => {
                            return Err(AgentError::Provider(e));
                        }
                    }
                }
            }
        }

        retry_count = 0;
        context_compacted = false;

        if let Some(response) = &final_response {
            if assistant_text.is_empty() {
                assistant_text = response
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ResponseContent::Text(text) => Some(text.as_str()),
                        ResponseContent::ToolUse { .. } | ResponseContent::HostedToolUse { .. } => {
                            None
                        }
                    })
                    .collect();
            }
            if tool_uses.is_empty() {
                tool_uses = response
                    .content
                    .iter()
                    .enumerate()
                    .filter_map(|(index, block)| match block {
                        ResponseContent::ToolUse { id, name, input } => Some((
                            index,
                            id.clone(),
                            name.clone(),
                            input.clone(),
                            String::new(),
                            false,
                        )),
                        ResponseContent::Text(_) | ResponseContent::HostedToolUse { .. } => None,
                    })
                    .collect();
            }
            for (index, block) in response.content.iter().enumerate() {
                if let ResponseContent::HostedToolUse {
                    id,
                    name,
                    input,
                    output,
                    status,
                } = block
                {
                    let id = normalize_hosted_tool_id(index, id.clone(), name);
                    let name = normalize_hosted_tool_name(name.clone());
                    let previous_input = hosted_tool_inputs
                        .get(&id)
                        .map(|(_, _, previous_input)| previous_input);
                    let input = hosted_tool_input_or_previous(input.clone(), previous_input);
                    hosted_tool_inputs.insert(id.clone(), (index, name.clone(), input.clone()));
                    emit_hosted_tool_start(
                        &emit,
                        &mut emitted_hosted_tool_starts,
                        &id,
                        &name,
                        &input,
                    );
                    if output.is_some() || status.is_some() {
                        emit_hosted_tool_result(
                            &emit,
                            &mut emitted_hosted_tool_results,
                            &session.cwd,
                            HostedToolResultEvent {
                                id: &id,
                                name: &name,
                                input: &input,
                                output: output.clone(),
                                status: status.clone(),
                            },
                        );
                    }
                }
            }
            if reasoning_text.is_empty() {
                let final_reasoning = response
                    .metadata
                    .extras
                    .iter()
                    .filter_map(|extra| match extra {
                        ResponseExtra::ReasoningText { text } => Some(text.as_str()),
                        ResponseExtra::ProviderSpecific { .. } => None,
                    })
                    .collect::<String>();
                if !final_reasoning.is_empty() {
                    emit(QueryEvent::ReasoningDelta(final_reasoning.clone()));
                    reasoning_text = final_reasoning;
                }
            }
        }

        let pending_hosted_tools = hosted_tool_inputs
            .iter()
            .map(|(id, (_index, name, input))| (id.clone(), name.clone(), input.clone()))
            .collect::<Vec<_>>();
        for (id, name, input) in pending_hosted_tools {
            emit_hosted_tool_start(&emit, &mut emitted_hosted_tool_starts, &id, &name, &input);
            emit_hosted_tool_result(
                &emit,
                &mut emitted_hosted_tool_results,
                &session.cwd,
                HostedToolResultEvent {
                    id: &id,
                    name: &name,
                    input: &input,
                    output: None,
                    status: Some("completed".to_string()),
                },
            );
        }

        // Build assistant message
        let mut assistant_content: Vec<ContentBlock> = Vec::new();

        if !reasoning_text.is_empty() {
            assistant_content.push(ContentBlock::Reasoning {
                text: reasoning_text,
            });
        }

        if !assistant_text.is_empty() {
            assistant_content.push(ContentBlock::Text {
                text: assistant_text,
            });
        }

        let final_tool_inputs: HashMap<String, serde_json::Value> = final_response
            .as_ref()
            .map(|response| {
                response
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ResponseContent::ToolUse { id, input, .. } => {
                            Some((id.clone(), input.clone()))
                        }
                        ResponseContent::Text(_) | ResponseContent::HostedToolUse { .. } => None,
                    })
                    .collect()
            })
            .unwrap_or_default();

        let tool_calls: Vec<ToolCall> = tool_uses
            .into_iter()
            .map(|(_index, id, name, initial_input, json_str, saw_delta)| {
                let input = if saw_delta {
                    serde_json::from_str(&json_str).unwrap_or_else(|_| {
                        final_tool_inputs.get(&id).cloned().unwrap_or(initial_input)
                    })
                } else {
                    final_tool_inputs.get(&id).cloned().unwrap_or(initial_input)
                };
                if emitted_tool_use_starts.insert(id.clone()) {
                    emit(QueryEvent::ToolUseStart {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    });
                }
                assistant_content.push(ContentBlock::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                });
                ToolCall { id, name, input }
            })
            .collect();

        session.push_message(Message {
            role: Role::Assistant,
            content: assistant_content,
        });

        // If no tool calls, check stop reason
        if tool_calls.is_empty() {
            // MaxOutputTokens auto-continue
            if stop_reason == Some(StopReason::MaxTokens) {
                debug!("max_tokens reached injecting continuation prompt");
                session.push_message(Message::user("Please continue from where you left off."));
                continue;
            }

            if let Some(sr) = stop_reason {
                emit(QueryEvent::TurnComplete { stop_reason: sr });
            }
            debug!("no tool calls, ending query loop");
            session.end_turn();
            return Ok(());
        }

        let tool_result_metadata: HashMap<String, (String, serde_json::Value, String)> = tool_calls
            .iter()
            .map(|call| {
                (
                    call.id.clone(),
                    (
                        call.name.clone(),
                        call.input.clone(),
                        crate::tools::tool_summary::tool_summary(
                            &call.name,
                            &call.input,
                            &session.cwd,
                        ),
                    ),
                )
            })
            .collect();

        // Execute tool calls. When a caller is observing query events, wire
        // tool progress and per-call completion into the same event stream so
        // long-running and parallel tools can render before the whole batch ends.
        let results = if let Some(progress_events) = on_event.clone() {
            let completion_events = Arc::clone(&progress_events);
            let metadata = Arc::new(tool_result_metadata.clone());
            runtime
                .execute_batch_streaming_with_completion(
                    &tool_calls,
                    move |tool_use_id, content| {
                        progress_events(QueryEvent::ToolProgress {
                            tool_use_id: tool_use_id.to_string(),
                            content: content.to_string(),
                        });
                    },
                    move |result| {
                        let content = compact_tool_content(result.content.clone());
                        let display_content = result.display_content.clone().map(micro_compact);
                        let (tool_name, input, summary) = metadata
                            .get(result.tool_use_id.as_str())
                            .cloned()
                            .unwrap_or_else(|| {
                                (String::new(), serde_json::Value::Null, String::new())
                            });
                        completion_events(QueryEvent::ToolResult {
                            tool_use_id: result.tool_use_id.clone(),
                            tool_name,
                            input,
                            content,
                            display_content,
                            is_error: result.is_error,
                            summary,
                        });
                    },
                )
                .await
        } else {
            runtime.execute_batch(&tool_calls).await
        };
        let tool_result_count = results.len();
        let tool_error_count = results.iter().filter(|result| result.is_error).count();
        let tool_output_bytes = results
            .iter()
            .map(|result| tool_content_model_bytes(&result.content))
            .sum::<usize>();
        debug!(
            tool_calls = tool_calls.len(),
            tool_results = tool_result_count,
            tool_errors = tool_error_count,
            tool_output_bytes,
            "tool batch completed"
        );

        // Build tool result message (user role, per Anthropic API convention)
        // Apply micro-compact to large tool results
        let result_content: Vec<ContentBlock> = results
            .into_iter()
            .map(|r| {
                let tool_name = tool_result_metadata
                    .get(r.tool_use_id.as_str())
                    .map(|(tool_name, _, _)| tool_name.as_str());
                let content_str = r.content.into_string();
                let compacted_content = if preserve_full_tool_result(tool_name) {
                    content_str
                } else {
                    micro_compact(content_str)
                };
                ContentBlock::ToolResult {
                    tool_use_id: r.tool_use_id,
                    content: compacted_content,
                    is_error: r.is_error,
                }
            })
            .collect();

        session.push_message(Message {
            role: Role::User,
            content: result_content,
        });
    }
}

/// Sends a minimal provider probe request used by onboarding and configuration checks.
pub async fn test_model_connection(
    provider: &dyn ModelProviderSDK,
    model: &Model,
    prompt: &str,
) -> Result<String, AgentError> {
    let ResolvedThinkingRequest {
        request_model,
        request_thinking,
        request_reasoning_effort,
        extra_body,
        effective_reasoning_effort: _,
    } = model.resolve_thinking_selection(None);
    let request = ModelRequest {
        model: request_model,
        system: None,
        messages: vec![devo_protocol::RequestMessage {
            role: "user".to_string(),
            content: vec![devo_protocol::RequestContent::Text {
                text: prompt.to_string(),
            }],
        }],
        max_tokens: model.max_tokens.map_or(64, |value| value as usize),
        tools: None,
        hosted_tools: Vec::new(),
        sampling: SamplingControls {
            temperature: model.temperature,
            top_p: model.top_p,
            top_k: model.top_k.map(|value| value as u32),
        },
        thinking: request_thinking,
        reasoning_effort: request_reasoning_effort,
        extra_body,
    };
    let mut stream = provider.completion_stream(request).await?;
    let mut reply_preview = String::new();
    while let Some(event) = stream.next().await {
        match event? {
            StreamEvent::TextDelta { text, .. } => reply_preview.push_str(&text),
            StreamEvent::MessageDone { response } => {
                if reply_preview.trim().is_empty() {
                    reply_preview = response
                        .content
                        .into_iter()
                        .find_map(|content| match content {
                            ResponseContent::Text(text) => Some(text),
                            _ => None,
                        })
                        .unwrap_or_default();
                }
                break;
            }
            _ => {}
        }
    }
    let preview = reply_preview.trim();
    if preview.is_empty() {
        return Err(AgentError::Provider(anyhow::anyhow!(
            "provider validation completed without a model reply"
        )));
    }
    Ok(preview.to_string())
}

fn retry_backoff_duration(attempt: usize) -> Duration {
    let exponent = attempt.saturating_sub(1).min(10) as u32;
    let multiplier = 2u64.pow(exponent);
    Duration::from_millis(INITIAL_RETRY_BACKOFF_MS.saturating_mul(multiplier))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use crate::tools::ToolAgentScope;
    use crate::tools::ToolContent;
    use crate::tools::ToolPreparationFeedback;
    use crate::tools::ToolRegistry;
    use crate::tools::ToolRuntime;
    use crate::tools::ToolRuntimeContext;
    use crate::tools::json_schema::JsonSchema;
    use crate::tools::registry::ToolExposure;
    use crate::tools::registry::ToolRegistryBuilder;
    use crate::tools::router::PermissionChecker;
    use crate::tools::tool_handler::ToolHandler;
    use crate::tools::tool_spec::ToolExecutionMode;
    use crate::tools::tool_spec::ToolOutputMode;
    use crate::tools::tool_spec::ToolSpec;
    use anyhow::Result;
    use async_trait::async_trait;
    use devo_protocol::CollaborationMode;
    use devo_protocol::ModelRequest;
    use devo_protocol::ModelResponse;
    use devo_protocol::RequestContent;
    use devo_protocol::RequestMessage;
    use devo_protocol::ResponseContent;
    use devo_protocol::ResponseExtra;
    use devo_protocol::ResponseMetadata;
    use devo_protocol::StopReason;
    use devo_protocol::StreamEvent;
    use devo_protocol::ThreadGoal;
    use devo_protocol::ThreadGoalStatus;
    use devo_protocol::Usage;
    use devo_provider::ModelProviderSDK;
    use devo_safety::PermissionMode;
    use futures::Stream;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::QueryEvent;
    use super::hosted_tools_for_web_search;
    use super::insert_subagent_request_reminders;
    use super::query;
    use super::test_model_connection;
    use crate::ContentBlock;
    use crate::Message;
    use crate::Model;
    use crate::ReasoningEffort;
    use crate::Role;

    #[test]
    fn hosted_tools_follow_resolved_web_search_mode() {
        let hosted = hosted_tools_for_web_search(&devo_config::ResolvedWebSearchConfig::Provider);
        assert_eq!(hosted.len(), 1);
        assert!(matches!(
            hosted.as_slice(),
            [devo_protocol::HostedToolDefinition::WebSearch(_)]
        ));

        assert_eq!(
            hosted_tools_for_web_search(&devo_config::ResolvedWebSearchConfig::Disabled),
            Vec::new()
        );
        assert_eq!(
            hosted_tools_for_web_search(&devo_config::ResolvedWebSearchConfig::Local(
                devo_config::ResolvedLocalWebSearchConfig {
                    provider_id: "test".to_string(),
                    kind: devo_config::LocalWebSearchProviderKind::Exa,
                    api_key: "secret".to_string(),
                    base_url: None,
                    max_results: None,
                },
            )),
            Vec::new()
        );
    }
    use crate::SessionConfig;
    use crate::SessionState;
    use crate::ThinkingCapability;
    use crate::ThinkingImplementation;
    use crate::ThinkingVariant;
    use crate::ThinkingVariantConfig;
    use crate::TruncationMode;
    use crate::TruncationPolicyConfig;
    use crate::TurnConfig;

    struct SingleToolUseProvider {
        requests: AtomicUsize,
    }

    struct InterleavedToolUseProvider {
        requests: AtomicUsize,
    }

    struct ParallelToolUseProvider {
        requests: AtomicUsize,
    }

    #[async_trait]
    impl devo_provider::ModelProviderSDK for SingleToolUseProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            unreachable!("tests stream responses only")
        }

        async fn completion_stream(
            &self,
            _request: ModelRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
            let request_number = self.requests.fetch_add(1, Ordering::SeqCst);

            let events = if request_number == 0 {
                vec![
                    Ok(StreamEvent::ToolCallStart {
                        index: 0,
                        id: "tool-1".into(),
                        name: "mutating_tool".into(),
                        input: json!({}),
                    }),
                    Ok(StreamEvent::ToolCallInputDelta {
                        index: 0,
                        partial_json: r#"{"value":1}"#.into(),
                    }),
                    Ok(StreamEvent::MessageDone {
                        response: ModelResponse {
                            id: "resp-1".into(),
                            content: vec![ResponseContent::ToolUse {
                                id: "tool-1".into(),
                                name: "mutating_tool".into(),
                                input: json!({ "value": 1 }),
                            }],
                            stop_reason: Some(StopReason::ToolUse),
                            usage: Usage::default(),
                            metadata: Default::default(),
                        },
                    }),
                ]
            } else {
                vec![
                    Ok(StreamEvent::TextDelta {
                        index: 0,
                        text: "done".into(),
                    }),
                    Ok(StreamEvent::MessageDone {
                        response: ModelResponse {
                            id: "resp-2".into(),
                            content: vec![ResponseContent::Text("done".into())],
                            stop_reason: Some(StopReason::EndTurn),
                            usage: Usage::default(),
                            metadata: Default::default(),
                        },
                    }),
                ]
            };

            Ok(Box::pin(futures::stream::iter(events)))
        }

        fn name(&self) -> &str {
            "test-provider"
        }
    }

    #[async_trait]
    impl devo_provider::ModelProviderSDK for InterleavedToolUseProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            unreachable!("tests stream responses only")
        }

        async fn completion_stream(
            &self,
            _request: ModelRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
            let request_number = self.requests.fetch_add(1, Ordering::SeqCst);

            let events = if request_number == 0 {
                vec![
                    Ok(StreamEvent::ToolCallStart {
                        index: 0,
                        id: "tool-1".into(),
                        name: "mutating_tool".into(),
                        input: json!({}),
                    }),
                    Ok(StreamEvent::ToolCallStart {
                        index: 1,
                        id: "tool-2".into(),
                        name: "mutating_tool".into(),
                        input: json!({}),
                    }),
                    Ok(StreamEvent::ToolCallInputDelta {
                        index: 0,
                        partial_json: r#"{"value":1}"#.into(),
                    }),
                    Ok(StreamEvent::ToolCallInputDelta {
                        index: 1,
                        partial_json: r#"{"value":2}"#.into(),
                    }),
                    Ok(StreamEvent::MessageDone {
                        response: ModelResponse {
                            id: "resp-1".into(),
                            content: vec![
                                ResponseContent::ToolUse {
                                    id: "tool-1".into(),
                                    name: "mutating_tool".into(),
                                    input: json!({}),
                                },
                                ResponseContent::ToolUse {
                                    id: "tool-2".into(),
                                    name: "mutating_tool".into(),
                                    input: json!({}),
                                },
                            ],
                            stop_reason: Some(StopReason::ToolUse),
                            usage: Usage::default(),
                            metadata: Default::default(),
                        },
                    }),
                ]
            } else {
                vec![
                    Ok(StreamEvent::TextDelta {
                        index: 0,
                        text: "done".into(),
                    }),
                    Ok(StreamEvent::MessageDone {
                        response: ModelResponse {
                            id: "resp-2".into(),
                            content: vec![ResponseContent::Text("done".into())],
                            stop_reason: Some(StopReason::EndTurn),
                            usage: Usage::default(),
                            metadata: Default::default(),
                        },
                    }),
                ]
            };

            Ok(Box::pin(futures::stream::iter(events)))
        }

        fn name(&self) -> &str {
            "interleaved-test-provider"
        }
    }

    #[async_trait]
    impl devo_provider::ModelProviderSDK for ParallelToolUseProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            unreachable!("tests stream responses only")
        }

        async fn completion_stream(
            &self,
            _request: ModelRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
            let request_number = self.requests.fetch_add(1, Ordering::SeqCst);

            let events = if request_number == 0 {
                vec![
                    Ok(StreamEvent::ToolCallStart {
                        index: 0,
                        id: "slow".into(),
                        name: "parallel_tool".into(),
                        input: json!({
                            "delay_ms": 50,
                            "output": "slow complete",
                        }),
                    }),
                    Ok(StreamEvent::ToolCallStart {
                        index: 1,
                        id: "fast".into(),
                        name: "parallel_tool".into(),
                        input: json!({
                            "delay_ms": 5,
                            "output": "fast complete",
                        }),
                    }),
                    Ok(StreamEvent::MessageDone {
                        response: ModelResponse {
                            id: "resp-1".into(),
                            content: vec![
                                ResponseContent::ToolUse {
                                    id: "slow".into(),
                                    name: "parallel_tool".into(),
                                    input: json!({
                                        "delay_ms": 50,
                                        "output": "slow complete",
                                    }),
                                },
                                ResponseContent::ToolUse {
                                    id: "fast".into(),
                                    name: "parallel_tool".into(),
                                    input: json!({
                                        "delay_ms": 5,
                                        "output": "fast complete",
                                    }),
                                },
                            ],
                            stop_reason: Some(StopReason::ToolUse),
                            usage: Usage::default(),
                            metadata: Default::default(),
                        },
                    }),
                ]
            } else {
                vec![Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp-2".into(),
                        content: vec![ResponseContent::Text("done".into())],
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Usage::default(),
                        metadata: Default::default(),
                    },
                })]
            };

            Ok(Box::pin(futures::stream::iter(events)))
        }

        fn name(&self) -> &str {
            "parallel-tool-provider"
        }
    }

    struct MutatingTool;

    struct CapturingProvider {
        requests: Arc<Mutex<Vec<ModelRequest>>>,
    }

    struct OpenAiCapturingProvider {
        requests: Arc<Mutex<Vec<ModelRequest>>>,
    }

    struct HostedWebSearchProvider {
        requests: Arc<Mutex<Vec<ModelRequest>>>,
    }

    struct HostedWebFetchProvider {
        requests: Arc<Mutex<Vec<ModelRequest>>>,
    }

    struct TransientStreamCreateProvider {
        attempts: AtomicUsize,
    }

    struct TransientStreamEventProvider {
        attempts: AtomicUsize,
    }

    #[async_trait]
    impl devo_provider::ModelProviderSDK for CapturingProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            unreachable!("tests stream responses only")
        }

        async fn completion_stream(
            &self,
            request: ModelRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
            self.requests.lock().expect("lock requests").push(request);
            Ok(Box::pin(futures::stream::iter(vec![Ok(
                StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp".into(),
                        content: vec![ResponseContent::Text("done".into())],
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Usage::default(),
                        metadata: Default::default(),
                    },
                },
            )])))
        }

        fn name(&self) -> &str {
            "capturing-provider"
        }
    }

    #[async_trait]
    impl devo_provider::ModelProviderSDK for OpenAiCapturingProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            unreachable!("tests stream responses only")
        }

        async fn completion_stream(
            &self,
            request: ModelRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
            self.requests.lock().expect("lock requests").push(request);
            Ok(Box::pin(futures::stream::iter(vec![Ok(
                StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp".into(),
                        content: vec![ResponseContent::Text("done".into())],
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Usage::default(),
                        metadata: Default::default(),
                    },
                },
            )])))
        }

        fn name(&self) -> &str {
            "openai"
        }
    }

    #[async_trait]
    impl devo_provider::ModelProviderSDK for HostedWebSearchProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            unreachable!("tests stream responses only")
        }

        async fn completion_stream(
            &self,
            request: ModelRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
            self.requests.lock().expect("lock requests").push(request);
            let input = json!({ "query": "current Rust docs" });
            let output = Some(json!({
                "results": [
                    {
                        "title": "Rust documentation",
                        "url": "https://example.test/rust"
                    }
                ]
            }));
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(StreamEvent::HostedToolCallStart {
                    index: 0,
                    id: "hosted_ws_1".into(),
                    name: "web_search".into(),
                    input: input.clone(),
                }),
                Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp".into(),
                        content: vec![
                            ResponseContent::HostedToolUse {
                                id: "hosted_ws_1".into(),
                                name: "web_search".into(),
                                input: input.clone(),
                                output: None,
                                status: None,
                            },
                            ResponseContent::HostedToolUse {
                                id: "hosted_ws_1".into(),
                                name: "web_search".into(),
                                input,
                                output,
                                status: Some("completed".into()),
                            },
                        ],
                        stop_reason: Some(StopReason::ToolUse),
                        usage: Usage::default(),
                        metadata: Default::default(),
                    },
                }),
            ])))
        }

        fn name(&self) -> &str {
            "hosted-web-search-provider"
        }
    }

    #[async_trait]
    impl devo_provider::ModelProviderSDK for HostedWebFetchProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            unreachable!("tests stream responses only")
        }

        async fn completion_stream(
            &self,
            request: ModelRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
            self.requests.lock().expect("lock requests").push(request);
            let input = json!({ "url": "https://example.test/docs" });
            let output = Some(json!({
                "title": "Docs",
                "url": "https://example.test/docs"
            }));
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(StreamEvent::HostedToolCallStart {
                    index: 0,
                    id: "hosted_wf_1".into(),
                    name: "web_fetch".into(),
                    input: input.clone(),
                }),
                Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp".into(),
                        content: vec![
                            ResponseContent::HostedToolUse {
                                id: "hosted_wf_1".into(),
                                name: "web_fetch".into(),
                                input: input.clone(),
                                output: None,
                                status: None,
                            },
                            ResponseContent::HostedToolUse {
                                id: "hosted_wf_1".into(),
                                name: "web_fetch".into(),
                                input,
                                output,
                                status: Some("completed".into()),
                            },
                        ],
                        stop_reason: Some(StopReason::ToolUse),
                        usage: Usage::default(),
                        metadata: Default::default(),
                    },
                }),
            ])))
        }

        fn name(&self) -> &str {
            "hosted-web-fetch-provider"
        }
    }

    #[async_trait]
    impl devo_provider::ModelProviderSDK for TransientStreamCreateProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            unreachable!("tests stream responses only")
        }

        async fn completion_stream(
            &self,
            _request: ModelRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
            let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                return Err(anyhow::anyhow!("503 service unavailable"));
            }

            Ok(Box::pin(futures::stream::iter(vec![Ok(
                StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp".into(),
                        content: vec![ResponseContent::Text("done".into())],
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Usage::default(),
                        metadata: Default::default(),
                    },
                },
            )])))
        }

        fn name(&self) -> &str {
            "transient-stream-create-provider"
        }
    }

    #[async_trait]
    impl devo_provider::ModelProviderSDK for TransientStreamEventProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            unreachable!("tests stream responses only")
        }

        async fn completion_stream(
            &self,
            _request: ModelRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
            let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                return Ok(Box::pin(futures::stream::iter(vec![Err(anyhow::anyhow!(
                    "500 internal server error"
                ))])));
            }

            Ok(Box::pin(futures::stream::iter(vec![Ok(
                StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp".into(),
                        content: vec![ResponseContent::Text("done".into())],
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Usage::default(),
                        metadata: Default::default(),
                    },
                },
            )])))
        }

        fn name(&self) -> &str {
            "transient-stream-event-provider"
        }
    }

    #[async_trait]
    impl ToolHandler for MutatingTool {
        fn spec(&self) -> &crate::tools::tool_spec::ToolSpec {
            // Leak a static spec for test purposes
            Box::leak(Box::new(crate::tools::tool_spec::ToolSpec::new(
                "write",
                "write tool",
                crate::tools::JsonSchema::object(Default::default(), None, None),
            )))
        }

        async fn handle(
            &self,
            _ctx: crate::tools::contracts::ToolContext,
            _input: serde_json::Value,
            _progress: Option<crate::tools::contracts::ToolProgressSender>,
        ) -> Result<crate::tools::contracts::ToolResult, crate::tools::contracts::ToolCallError>
        {
            Ok(crate::tools::contracts::ToolResult::success(
                crate::tools::contracts::ToolResultContent::Text("ok".into()),
                "ok",
            ))
        }
    }

    struct DisplayContentTool;

    struct CountingWebSearchTool {
        executions: Arc<AtomicUsize>,
    }

    struct CountingWebFetchTool {
        executions: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ToolHandler for CountingWebSearchTool {
        fn spec(&self) -> &crate::tools::tool_spec::ToolSpec {
            Box::leak(Box::new(crate::tools::tool_spec::ToolSpec::new(
                "web_search",
                "Search the web.",
                crate::tools::JsonSchema::object(Default::default(), None, None),
            )))
        }

        async fn handle(
            &self,
            _ctx: crate::tools::contracts::ToolContext,
            _input: serde_json::Value,
            _progress: Option<crate::tools::contracts::ToolProgressSender>,
        ) -> Result<crate::tools::contracts::ToolResult, crate::tools::contracts::ToolCallError>
        {
            self.executions.fetch_add(1, Ordering::SeqCst);
            Ok(crate::tools::contracts::ToolResult::success(
                crate::tools::contracts::ToolResultContent::Text("local search".into()),
                "local search",
            ))
        }
    }

    #[async_trait]
    impl ToolHandler for CountingWebFetchTool {
        fn spec(&self) -> &crate::tools::tool_spec::ToolSpec {
            Box::leak(Box::new(crate::tools::tool_spec::ToolSpec::new(
                "webfetch",
                "Fetch a URL.",
                crate::tools::JsonSchema::object(Default::default(), None, None),
            )))
        }

        async fn handle(
            &self,
            _ctx: crate::tools::contracts::ToolContext,
            _input: serde_json::Value,
            _progress: Option<crate::tools::contracts::ToolProgressSender>,
        ) -> Result<crate::tools::contracts::ToolResult, crate::tools::contracts::ToolCallError>
        {
            self.executions.fetch_add(1, Ordering::SeqCst);
            Ok(crate::tools::contracts::ToolResult::success(
                crate::tools::contracts::ToolResultContent::Text("local fetch".into()),
                "local fetch",
            ))
        }
    }

    #[async_trait]
    impl ToolHandler for DisplayContentTool {
        fn spec(&self) -> &crate::tools::tool_spec::ToolSpec {
            Box::leak(Box::new(crate::tools::tool_spec::ToolSpec::new(
                "read",
                "read tool",
                crate::tools::JsonSchema::object(Default::default(), None, None),
            )))
        }

        async fn handle(
            &self,
            _ctx: crate::tools::contracts::ToolContext,
            _input: serde_json::Value,
            _progress: Option<crate::tools::contracts::ToolProgressSender>,
        ) -> Result<crate::tools::contracts::ToolResult, crate::tools::contracts::ToolCallError>
        {
            let mut result = crate::tools::contracts::ToolResult::success(
                crate::tools::contracts::ToolResultContent::Text("canonical".into()),
                "done",
            );
            result.display_content = Some("display".to_string());
            Ok(result)
        }
    }

    struct StreamingMutatingTool;

    struct ParallelDelayTool;

    #[async_trait]
    impl ToolHandler for StreamingMutatingTool {
        fn spec(&self) -> &crate::tools::tool_spec::ToolSpec {
            Box::leak(Box::new(crate::tools::tool_spec::ToolSpec::new(
                "write",
                "write tool",
                crate::tools::JsonSchema::object(Default::default(), None, None),
            )))
        }

        async fn handle(
            &self,
            _ctx: crate::tools::contracts::ToolContext,
            _input: serde_json::Value,
            _progress: Option<crate::tools::contracts::ToolProgressSender>,
        ) -> Result<crate::tools::contracts::ToolResult, crate::tools::contracts::ToolCallError>
        {
            Ok(crate::tools::contracts::ToolResult::success(
                crate::tools::contracts::ToolResultContent::Text("stream complete".into()),
                "done",
            ))
        }
    }

    #[async_trait]
    impl ToolHandler for ParallelDelayTool {
        fn spec(&self) -> &crate::tools::tool_spec::ToolSpec {
            Box::leak(Box::new(crate::tools::tool_spec::ToolSpec::new(
                "read",
                "read tool",
                crate::tools::JsonSchema::object(Default::default(), None, None),
            )))
        }

        async fn handle(
            &self,
            _ctx: crate::tools::contracts::ToolContext,
            input: serde_json::Value,
            _progress: Option<crate::tools::contracts::ToolProgressSender>,
        ) -> Result<crate::tools::contracts::ToolResult, crate::tools::contracts::ToolCallError>
        {
            let delay_ms = input
                .get("delay_ms")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
            let output = input
                .get("output")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            Ok(crate::tools::contracts::ToolResult::success(
                crate::tools::contracts::ToolResultContent::Text(output.to_string()),
                "done",
            ))
        }
    }

    #[tokio::test]
    async fn query_retries_transient_stream_creation_errors() {
        let provider = Arc::new(TransientStreamCreateProvider {
            attempts: AtomicUsize::new(0),
        });
        let provider_sdk: Arc<dyn ModelProviderSDK> = provider.clone();
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("hello"));

        query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            provider_sdk,
            registry,
            &runtime,
            None,
        )
        .await
        .expect("query should retry and succeed");

        assert_eq!(provider.attempts.load(Ordering::SeqCst), 2);
        assert_eq!(
            session.messages.last(),
            Some(&Message::assistant_text("done"))
        );
    }

    #[tokio::test]
    async fn query_retries_transient_stream_event_errors_before_content() {
        let provider = Arc::new(TransientStreamEventProvider {
            attempts: AtomicUsize::new(0),
        });
        let provider_sdk: Arc<dyn ModelProviderSDK> = provider.clone();
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("hello"));

        query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            provider_sdk,
            registry,
            &runtime,
            None,
        )
        .await
        .expect("query should retry and succeed");

        assert_eq!(provider.attempts.load(Ordering::SeqCst), 2);
        assert_eq!(
            session.messages.last(),
            Some(&Message::assistant_text("done"))
        );
    }

    #[tokio::test]
    async fn query_exposes_stable_tools_and_appends_subagent_warning() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
            requests: Arc::clone(&requests),
        });
        let mut builder = ToolRegistryBuilder::new();
        builder.push_spec_with_exposure(
            ToolSpec::new(
                "ToolSearch",
                "Search available tools.",
                JsonSchema::object(Default::default(), None, None),
            ),
            ToolExposure::Direct,
        );
        builder.push_spec_with_exposure(
            ToolSpec::new(
                "web_search",
                "Search the web.",
                JsonSchema::object(Default::default(), None, None),
            ),
            ToolExposure::Deferred,
        );
        for (name, description) in [
            ("spawn_agent", "Create a child agent."),
            ("send_message", "Send input to a child agent."),
            ("wait_agent", "Poll child output."),
            ("list_agents", "List child agents."),
            ("close_agent", "Close a child agent."),
        ] {
            builder.push_spec_with_exposure(
                ToolSpec::new(
                    name,
                    description,
                    JsonSchema::object(Default::default(), None, None),
                ),
                ToolExposure::Deferred,
            );
        }
        let registry = Arc::new(builder.build());
        let runtime = ToolRuntime::new_with_context(
            Arc::clone(&registry),
            PermissionChecker::always_allow(),
            ToolRuntimeContext {
                agent_scope: ToolAgentScope::Subagent,
                ..ToolRuntimeContext::default()
            },
        );
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("work on the delegated task"));
        {
            let loaded_deferred_tools = registry.loaded_deferred_tools();
            let mut loaded_deferred_tools = loaded_deferred_tools.lock().expect("loaded tools");
            for name in [
                "spawn_agent",
                "send_message",
                "wait_agent",
                "list_agents",
                "close_agent",
            ] {
                loaded_deferred_tools.mark_loaded(&session.id, name);
            }
        }

        let mut turn_config = TurnConfig::new(
            Model {
                base_instructions: "base system".to_string(),
                ..Model::default()
            },
            None,
        );
        turn_config.web_search = devo_config::ResolvedWebSearchConfig::Local(
            devo_config::ResolvedLocalWebSearchConfig {
                provider_id: "test".to_string(),
                kind: devo_config::LocalWebSearchProviderKind::Exa,
                api_key: "secret".to_string(),
                base_url: None,
                max_results: None,
            },
        );

        query(
            &mut session,
            &turn_config,
            provider,
            registry,
            &runtime,
            None,
        )
        .await
        .expect("query should complete");

        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 1);
        let request = &captured[0];
        let tool_names = request
            .tools
            .as_ref()
            .expect("tools should be present")
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(tool_names, vec!["ToolSearch", "web_search"]);
        let system = request.system.as_deref().expect("system prompt");
        let mode_prompt = crate::collaboration_mode_prompts::mode_introductions_prompt();
        assert!(system.contains("base system"));
        assert!(system.contains(&mode_prompt));
        assert!(system.contains("Sources:"));
        assert!(
            !request
                .system
                .as_deref()
                .unwrap_or_default()
                .contains("web_search")
        );
        assert!(
            !request
                .system
                .as_deref()
                .unwrap_or_default()
                .contains("spawn_agent")
        );

        assert!(
            request
                .messages
                .iter()
                .all(|message| !message_contains(message, "web_search: Search the web."))
        );
        let subagent_reminder_index =
            request_message_index_containing(request, "You are running as a sub-agent");
        let task_index = request_message_index_containing(request, "work on the delegated task");
        assert!(subagent_reminder_index < task_index);
        assert!(
            request
                .messages
                .iter()
                .any(|message| message_contains(message, "<context_changes>"))
        );
    }

    #[tokio::test]
    async fn query_adds_web_search_prompt_for_provider_hosted_search() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
            requests: Arc::clone(&requests),
        });
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("search current docs"));
        let mut turn_config = TurnConfig::new(
            Model {
                base_instructions: "base system".to_string(),
                ..Model::default()
            },
            None,
        );
        turn_config.web_search = devo_config::ResolvedWebSearchConfig::Provider;

        query(
            &mut session,
            &turn_config,
            provider,
            registry,
            &runtime,
            None,
        )
        .await
        .expect("query should complete");

        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 1);
        let request = &captured[0];
        let system = request.system.as_deref().expect("system prompt");

        assert!(system.contains("base system"));
        assert!(system.contains("Sources:"));
        assert!(system.contains("The current month is "));
        assert!(matches!(
            request.hosted_tools.as_slice(),
            [devo_protocol::HostedToolDefinition::WebSearch(_)]
        ));
        assert!(
            request
                .tools
                .as_ref()
                .is_none_or(|tools| tools.iter().all(|tool| tool.name != "web_search"))
        );
    }

    #[tokio::test]
    async fn provider_hosted_web_search_emits_tool_events_without_local_execution() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(HostedWebSearchProvider {
            requests: Arc::clone(&requests),
        });
        let executions = Arc::new(AtomicUsize::new(0));
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler(
            "web_search",
            Arc::new(CountingWebSearchTool {
                executions: Arc::clone(&executions),
            }),
        );
        builder.push_spec(ToolSpec {
            name: "web_search".into(),
            description: "Search the web.".into(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = Arc::new(builder.build());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("search current docs"));
        let mut turn_config = TurnConfig::new(Model::default(), None);
        turn_config.web_search = devo_config::ResolvedWebSearchConfig::Provider;

        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_clone = Arc::clone(&seen);
        let callback = Arc::new(move |event: QueryEvent| {
            seen_clone.lock().unwrap().push(event);
        });

        query(
            &mut session,
            &turn_config,
            provider,
            registry,
            &runtime,
            Some(callback),
        )
        .await
        .expect("query should complete");

        assert_eq!(executions.load(Ordering::SeqCst), 0);
        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 1);
        let request = &captured[0];
        assert!(matches!(
            request.hosted_tools.as_slice(),
            [devo_protocol::HostedToolDefinition::WebSearch(_)]
        ));
        assert!(
            request
                .tools
                .as_ref()
                .is_none_or(|tools| tools.iter().all(|tool| tool.name != "web_search"))
        );

        let events = seen.lock().unwrap();
        let starts = events
            .iter()
            .filter_map(|event| match event {
                QueryEvent::ToolUseStart { id, name, input } => {
                    Some((id.as_str(), name.as_str(), input.clone()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            starts,
            vec![(
                "hosted_ws_1",
                "web_search",
                json!({ "query": "current Rust docs" })
            )]
        );
        let results = events
            .iter()
            .filter_map(|event| match event {
                QueryEvent::ToolResult {
                    tool_use_id,
                    tool_name,
                    input,
                    content,
                    is_error,
                    ..
                } => Some((
                    tool_use_id.as_str(),
                    tool_name.as_str(),
                    input.clone(),
                    content,
                    *is_error,
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(results.len(), 1);
        let (tool_use_id, tool_name, input, content, is_error) = &results[0];
        assert_eq!(*tool_use_id, "hosted_ws_1");
        assert_eq!(*tool_name, "web_search");
        assert_eq!(input, &json!({ "query": "current Rust docs" }));
        assert!(!*is_error);
        assert!(matches!(
            *content,
            ToolContent::Text(text) if text == "status: completed"
        ));
        assert!(events.iter().any(|event| matches!(
            event,
            QueryEvent::TurnComplete {
                stop_reason: StopReason::ToolUse
            }
        )));
        assert!(session.messages.iter().all(|message| {
            message.content.iter().all(|block| {
                !matches!(
                    block,
                    ContentBlock::ToolUse { .. } | ContentBlock::ToolResult { .. }
                )
            })
        }));
    }

    #[tokio::test]
    async fn provider_hosted_web_fetch_emits_tool_events_without_local_execution() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(HostedWebFetchProvider {
            requests: Arc::clone(&requests),
        });
        let executions = Arc::new(AtomicUsize::new(0));
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler(
            "webfetch",
            Arc::new(CountingWebFetchTool {
                executions: Arc::clone(&executions),
            }),
        );
        builder.push_spec(ToolSpec {
            name: "webfetch".into(),
            description: "Fetch a URL.".into(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Mixed,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = Arc::new(builder.build());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("fetch docs"));
        let mut turn_config = TurnConfig::new(Model::default(), None);
        turn_config.web_fetch = devo_config::ResolvedWebFetchConfig::Provider;

        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_clone = Arc::clone(&seen);
        let callback = Arc::new(move |event: QueryEvent| {
            seen_clone.lock().unwrap().push(event);
        });

        query(
            &mut session,
            &turn_config,
            provider,
            registry,
            &runtime,
            Some(callback),
        )
        .await
        .expect("query should complete");

        assert_eq!(executions.load(Ordering::SeqCst), 0);
        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 1);
        let request = &captured[0];
        assert!(matches!(
            request.hosted_tools.as_slice(),
            [devo_protocol::HostedToolDefinition::WebFetch(_)]
        ));
        assert!(
            request
                .tools
                .as_ref()
                .is_none_or(|tools| tools.iter().all(|tool| tool.name != "webfetch"))
        );

        let events = seen.lock().unwrap();
        let starts = events
            .iter()
            .filter_map(|event| match event {
                QueryEvent::ToolUseStart { id, name, input } => {
                    Some((id.as_str(), name.as_str(), input.clone()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            starts,
            vec![(
                "hosted_wf_1",
                "web_fetch",
                json!({ "url": "https://example.test/docs" })
            )]
        );
        let results = events
            .iter()
            .filter_map(|event| match event {
                QueryEvent::ToolResult {
                    tool_use_id,
                    tool_name,
                    input,
                    content,
                    is_error,
                    ..
                } => Some((
                    tool_use_id.as_str(),
                    tool_name.as_str(),
                    input.clone(),
                    content,
                    *is_error,
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(results.len(), 1);
        let (tool_use_id, tool_name, input, content, is_error) = &results[0];
        assert_eq!(*tool_use_id, "hosted_wf_1");
        assert_eq!(*tool_name, "web_fetch");
        assert_eq!(input, &json!({ "url": "https://example.test/docs" }));
        assert!(!*is_error);
        assert!(matches!(
            *content,
            ToolContent::Text(text) if text == "status: completed"
        ));
    }

    #[test]
    fn subagent_reminder_insertion_preserves_tool_result_adjacency() {
        let mut messages = vec![
            RequestMessage {
                role: Role::User.as_str().to_string(),
                content: vec![RequestContent::Text {
                    text: "child task input".to_string(),
                }],
            },
            RequestMessage {
                role: Role::Assistant.as_str().to_string(),
                content: vec![RequestContent::ToolUse {
                    id: "tool-1".to_string(),
                    name: "read".to_string(),
                    input: json!({}),
                }],
            },
            RequestMessage {
                role: Role::User.as_str().to_string(),
                content: vec![RequestContent::ToolResult {
                    tool_use_id: "tool-1".to_string(),
                    content: "tool output".to_string(),
                    is_error: None,
                }],
            },
        ];

        insert_subagent_request_reminders(&mut messages);

        assert!(message_contains(
            &messages[0],
            "You are running as a sub-agent"
        ));
        assert!(message_contains(&messages[1], "child task input"));
        assert!(
            matches!(messages[2].content.as_slice(), [RequestContent::ToolUse { id, .. }] if id == "tool-1")
        );
        assert!(
            matches!(messages[3].content.as_slice(), [RequestContent::ToolResult { tool_use_id, .. }] if tool_use_id == "tool-1")
        );
    }

    fn request_message_index_containing(request: &ModelRequest, needle: &str) -> usize {
        request
            .messages
            .iter()
            .position(|message| message_contains(message, needle))
            .unwrap_or_else(|| {
                panic!("expected request message containing {needle:?}: {request:?}")
            })
    }

    fn message_contains(message: &RequestMessage, needle: &str) -> bool {
        message.content.iter().any(
            |content| matches!(content, RequestContent::Text { text } if text.contains(needle)),
        )
    }

    fn active_goal(objective: &str) -> ThreadGoal {
        ThreadGoal {
            thread_id: devo_protocol::SessionId::new(),
            objective: objective.to_string(),
            status: ThreadGoalStatus::Active,
            token_budget: Some(10_000),
            tokens_used: 250,
            time_used_seconds: 0,
            created_at: 1,
            updated_at: 1,
        }
    }

    #[tokio::test]
    async fn query_uses_session_permission_mode_for_mutating_tools() {
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler("mutating_tool", Arc::new(MutatingTool));
        builder.push_spec(ToolSpec {
            name: "mutating_tool".into(),
            description: "A test-only mutating tool.".into(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::Mutating,
            capability_tags: vec![],
            supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = Arc::new(builder.build());
        let deny_checker = PermissionChecker::new(|request| {
            let n = request.tool_name;
            Box::pin(async move { Err(format!("{n} denied")) })
        });
        let runtime = ToolRuntime::new(Arc::clone(&registry), deny_checker);

        let mut session = SessionState::new(
            SessionConfig {
                permission_mode: PermissionMode::Deny,
                ..Default::default()
            },
            std::env::temp_dir(),
        );
        session.push_message(Message::user("run the tool"));

        query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            Arc::new(SingleToolUseProvider {
                requests: AtomicUsize::new(0),
            }),
            registry,
            &runtime,
            None,
        )
        .await
        .expect("query should complete and append a tool_result");

        let tool_result_message = session
            .messages
            .iter()
            .find(|message| {
                message
                    .content
                    .iter()
                    .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
            })
            .expect("tool_result message should be appended");
        let ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } = &tool_result_message.content[0]
        else {
            panic!("expected tool_result content block");
        };

        assert_eq!(tool_use_id, "tool-1");
        assert!(
            *is_error,
            "denied permission should surface as a tool error"
        );
        assert!(
            content.contains("permission denied"),
            "expected tool_result to mention permission denial, got: {content}"
        );
    }

    #[tokio::test]
    async fn query_resolves_model_variant_thinking_before_building_request() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
            requests: Arc::clone(&requests),
        });
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let model = Model {
            slug: "kimi-k2.5".into(),
            display_name: "Kimi K2.5".into(),
            provider: devo_protocol::ProviderWireApi::OpenAIChatCompletions,
            description: None,
            thinking_capability: ThinkingCapability::Toggle,
            default_reasoning_effort: Some(ReasoningEffort::Medium),
            thinking_implementation: Some(ThinkingImplementation::ModelVariant(
                ThinkingVariantConfig {
                    variants: vec![
                        ThinkingVariant {
                            selection_value: "disabled".into(),
                            model_slug: "kimi-k2.5".into(),
                            reasoning_effort: None,
                            label: "Off".into(),
                            description: "Use the standard model".into(),
                        },
                        ThinkingVariant {
                            selection_value: "enabled".into(),
                            model_slug: "kimi-k2.5-thinking".into(),
                            reasoning_effort: Some(ReasoningEffort::Medium),
                            label: "On".into(),
                            description: "Use the thinking model".into(),
                        },
                    ],
                },
            )),
            base_instructions: String::new(),
            context_window: 200_000,
            effective_context_window_percent: None,
            truncation_policy: TruncationPolicyConfig {
                mode: TruncationMode::Tokens,
                limit: 10_000,
            },
            input_modalities: vec![],
            supports_image_detail_original: false,
            channel: None,
            temperature: None,
            top_p: None,
            top_k: None,
            max_tokens: None,
        };
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("hello"));

        query(
            &mut session,
            &TurnConfig::with_request_model(
                model,
                "vendor/kimi-k2.5".into(),
                HashMap::from([(
                    "kimi-k2.5-thinking".into(),
                    "vendor/kimi-k2.5-thinking".into(),
                )])
                .into(),
                Some("enabled".into()),
            ),
            Arc::clone(&provider),
            registry,
            &runtime,
            None,
        )
        .await
        .expect("query should succeed");

        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].model, "vendor/kimi-k2.5-thinking");
        assert_eq!(captured[0].thinking, None);
    }

    #[tokio::test]
    async fn query_sends_turn_config_request_model_to_provider() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
            requests: Arc::clone(&requests),
        });
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let model = Model {
            slug: "catalog-slug".into(),
            display_name: "Catalog Model".into(),
            base_instructions: "catalog instructions".into(),
            ..Model::default()
        };
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("hello"));

        query(
            &mut session,
            &TurnConfig::with_request_model(
                model,
                "vendor/model-name".into(),
                HashMap::new().into(),
                /*thinking_selection*/ None,
            ),
            Arc::clone(&provider),
            registry,
            &runtime,
            None,
        )
        .await
        .expect("query should succeed");

        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].model, "vendor/model-name");
        assert_eq!(
            session
                .session_context
                .as_ref()
                .expect("session context")
                .model
                .slug,
            "catalog-slug"
        );
    }

    /// Trace: L2-DES-CONTEXT-001
    /// Verifies: Plan turns append the active Plan collaboration prompt to the provider system prompt.
    #[tokio::test]
    async fn query_appends_plan_mode_reminder_to_system_prompt() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
            requests: Arc::clone(&requests),
        });
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let model = Model {
            slug: "model-a".into(),
            base_instructions: "base instructions".into(),
            ..Model::default()
        };
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.collaboration_mode = CollaborationMode::Plan;
        session.push_message(Message::user("plan this"));

        query(
            &mut session,
            &TurnConfig::new(model, None),
            Arc::clone(&provider),
            registry,
            &runtime,
            None,
        )
        .await
        .expect("query should succeed");

        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 1);
        let system = captured[0].system.as_deref().expect("system prompt");
        let mode_prompt = crate::collaboration_mode_prompts::mode_introductions_prompt();
        assert!(system.contains("base instructions"));
        assert!(system.contains(&mode_prompt));
        let mode_index = request_message_index_containing(&captured[0], "<collaboration_mode>");
        assert!(message_contains(
            &captured[0].messages[mode_index],
            "<current>plan</current>"
        ));
    }

    /// Trace: L2-DES-CONTEXT-001
    /// Verifies: Returning from Plan to Build uses Build system prompt and a lightweight mode diff.
    #[tokio::test]
    async fn query_inserts_mode_change_prompt_when_returning_to_build_mode() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
            requests: Arc::clone(&requests),
        });
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let model = Model {
            slug: "model-a".into(),
            base_instructions: "base instructions".into(),
            ..Model::default()
        };
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.collaboration_mode = CollaborationMode::Plan;
        session.push_message(Message::user("plan this"));

        query(
            &mut session,
            &TurnConfig::new(model.clone(), None),
            Arc::clone(&provider),
            Arc::clone(&registry),
            &runtime,
            None,
        )
        .await
        .expect("plan query should succeed");

        session.collaboration_mode = CollaborationMode::Build;
        session.push_message(Message::user("implement this"));
        query(
            &mut session,
            &TurnConfig::new(model, None),
            Arc::clone(&provider),
            registry,
            &runtime,
            None,
        )
        .await
        .expect("build query should succeed");

        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 2);
        assert_eq!(captured[0].system, captured[1].system);
        let system = captured[1].system.as_deref().expect("system prompt");
        let mode_prompt = crate::collaboration_mode_prompts::mode_introductions_prompt();
        assert!(system.contains("base instructions"));
        assert!(system.contains(&mode_prompt));

        let mode_change_index = request_message_index_containing(
            &captured[1],
            "<transition>plan -> build</transition>",
        );
        let request_index = request_message_index_containing(&captured[1], "implement this");
        assert!(mode_change_index < request_index);
        assert!(message_contains(
            &captured[1].messages[mode_change_index],
            "<previous>plan</previous>"
        ));
        assert!(message_contains(
            &captured[1].messages[mode_change_index],
            "<current>build</current>"
        ));
        assert!(message_contains(
            &captured[1].messages[mode_change_index],
            "<note>any previous instructions for other modes (e.g. Plan mode) are no longer active.</note>"
        ));
        assert!(!message_contains(
            &captured[1].messages[mode_change_index],
            "<collaboration_mode_build>"
        ));
        assert!(!message_contains(
            &captured[1].messages[mode_change_index],
            "<collaboration_mode_plan>"
        ));
    }

    #[tokio::test]
    async fn query_inserts_goal_context_before_latest_user_request() {
        // Trace: L2-DES-GOAL-001
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
            requests: Arc::clone(&requests),
        });
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let model = Model {
            slug: "model-a".into(),
            base_instructions: "base instructions".into(),
            ..Model::default()
        };
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.set_active_goal(active_goal("ship /goal"));
        session.push_message(Message::user("finish implementation"));

        query(
            &mut session,
            &TurnConfig::new(model, None),
            Arc::clone(&provider),
            registry,
            &runtime,
            None,
        )
        .await
        .expect("query should succeed");

        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 1);
        assert!(
            !captured[0]
                .system
                .as_deref()
                .unwrap_or_default()
                .contains("ship /goal")
        );
        let messages = &captured[0].messages;
        let goal_index = messages
            .iter()
            .position(|message| message_contains(message, "ship /goal"))
            .expect("goal context message");
        let request_index = messages
            .iter()
            .position(|message| message_contains(message, "finish implementation"))
            .expect("latest user request message");
        assert!(goal_index < request_index);
    }

    #[tokio::test]
    async fn autonomous_goal_context_is_latest_request_after_completed_turn() {
        // Trace: L2-DES-GOAL-001
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
            requests: Arc::clone(&requests),
        });
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let model = Model {
            slug: "model-a".into(),
            base_instructions: "base instructions".into(),
            ..Model::default()
        };
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.set_active_goal(active_goal("continue the active goal"));
        session.push_message(Message::user("older user prompt"));
        session.push_message(Message::assistant_text("older assistant reply"));

        query(
            &mut session,
            &TurnConfig::new(model, None),
            Arc::clone(&provider),
            registry,
            &runtime,
            None,
        )
        .await
        .expect("query should succeed");

        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 1);
        let messages = &captured[0].messages;
        let goal_index = messages
            .iter()
            .position(|message| message_contains(message, "continue the active goal"))
            .expect("goal context message");
        let assistant_index = messages
            .iter()
            .position(|message| message_contains(message, "older assistant reply"))
            .expect("assistant history message");
        assert!(goal_index > assistant_index);
        assert_eq!(goal_index, messages.len() - 1);
    }

    #[tokio::test]
    async fn query_locks_system_prompt_and_environment_prefix_per_session() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
            requests: Arc::clone(&requests),
        });
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let temp_root =
            std::env::temp_dir().join(format!("devo-query-lock-{}", uuid::Uuid::new_v4()));
        let second_cwd = temp_root.join("nested");
        let first_model = Model {
            slug: "model-a".into(),
            base_instructions: "base-a".into(),
            ..Model::default()
        };
        let second_model = Model {
            slug: "model-b".into(),
            base_instructions: "base-b".into(),
            ..Model::default()
        };

        let mut session = SessionState::new(SessionConfig::default(), temp_root.clone());
        session.push_message(Message::user("hello"));

        query(
            &mut session,
            &TurnConfig::new(first_model, None),
            Arc::clone(&provider),
            Arc::clone(&registry),
            &runtime,
            None,
        )
        .await
        .expect("first query should succeed");

        session.cwd = second_cwd;
        session.push_message(Message::user("follow up"));

        query(
            &mut session,
            &TurnConfig::new(second_model, Some("enabled".into())),
            Arc::clone(&provider),
            registry,
            &runtime,
            None,
        )
        .await
        .expect("second query should succeed");

        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 2);
        let mode_prompt = crate::collaboration_mode_prompts::mode_introductions_prompt();
        let expected_system = format!("base-a\n\n{mode_prompt}");
        assert_eq!(
            captured[0].system.as_deref(),
            Some(expected_system.as_str())
        );
        assert_eq!(
            captured[1].system.as_deref(),
            Some(expected_system.as_str())
        );

        let first_prefix = &captured[0].messages[0];
        let second_prefix = &captured[1].messages[0];
        assert_eq!(first_prefix.role, second_prefix.role);
        let devo_protocol::RequestContent::Text { text: first_text } = &first_prefix.content[0]
        else {
            panic!("expected text prefix");
        };
        let devo_protocol::RequestContent::Text { text: second_text } = &second_prefix.content[0]
        else {
            panic!("expected text prefix");
        };
        assert_eq!(first_text, second_text);
    }

    #[tokio::test]
    async fn query_inserts_context_diff_before_changed_turn_input() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
            requests: Arc::clone(&requests),
        });
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        let first_model = Model {
            slug: "model-a".into(),
            ..Model::default()
        };
        let second_model = Model {
            slug: "model-b".into(),
            ..Model::default()
        };

        session.push_message(Message::user("hello"));
        query(
            &mut session,
            &TurnConfig::new(first_model, None),
            Arc::clone(&provider),
            Arc::clone(&registry),
            &runtime,
            None,
        )
        .await
        .expect("first query should succeed");

        session.push_message(Message::user("follow up"));
        query(
            &mut session,
            &TurnConfig::new(second_model, Some("enabled".into())),
            Arc::clone(&provider),
            registry,
            &runtime,
            None,
        )
        .await
        .expect("second query should succeed");

        let diff_message = &session.messages[session.messages.len() - 3];
        let user_message = &session.messages[session.messages.len() - 2];
        assert_eq!(user_message, &Message::user("follow up"));
        let ContentBlock::Text { text } = &diff_message.content[0] else {
            panic!("expected text diff message");
        };
        assert!(text.contains("<context_changes>"));
        assert!(text.contains("<metadata>"));
        assert!(text.contains("<name>model</name>"));
        assert!(text.contains("<previous>model-a</previous>"));
        assert!(text.contains("<current>model-b</current>"));
    }

    #[tokio::test]
    async fn query_drops_orphaned_tool_calls_from_prompt_history() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
            requests: Arc::clone(&requests),
        });
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());

        session.push_message(Message::user("first"));
        session.push_message(Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Text {
                    text: "Calling tool".into(),
                },
                ContentBlock::ToolUse {
                    id: "call-1".into(),
                    name: "bash".into(),
                    input: json!({ "cmd": "pwd" }),
                },
            ],
        });
        session.push_message(Message::user("follow up"));

        query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            provider,
            registry,
            &runtime,
            None,
        )
        .await
        .expect("query should succeed");

        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 1);
        assert!(
            captured[0]
                .messages
                .iter()
                .flat_map(|message| message.content.iter())
                .all(|content| !matches!(content, devo_protocol::RequestContent::ToolUse { .. })),
            "expected orphaned tool calls to be removed from prompt history"
        );
    }

    #[tokio::test]
    async fn test_model_connection_sends_minimal_request() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = CapturingProvider {
            requests: Arc::clone(&requests),
        };
        let model = Model {
            slug: "glm-4.5".into(),
            top_p: Some(0.95),
            ..Model::default()
        };
        let preview = test_model_connection(&provider, &model, "Reply with OK only.")
            .await
            .expect("probe request should succeed");

        let captured = requests.lock().expect("lock requests");
        assert_eq!(preview, "done");
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].system, None);
        assert!(captured[0].tools.is_none());
        assert_eq!(captured[0].messages.len(), 1);
        assert_eq!(captured[0].sampling.top_p, Some(0.95));
    }

    #[tokio::test]
    async fn query_emits_reasoning_without_polluting_assistant_message_content() {
        struct ReasoningProvider;

        #[async_trait]
        impl devo_provider::ModelProviderSDK for ReasoningProvider {
            async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
                unreachable!("tests stream responses only")
            }

            async fn completion_stream(
                &self,
                _request: ModelRequest,
            ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
                Ok(Box::pin(futures::stream::iter(vec![
                    Ok(StreamEvent::ReasoningStart { index: 0 }),
                    Ok(StreamEvent::ReasoningDelta {
                        index: 0,
                        text: "plan".into(),
                    }),
                    Ok(StreamEvent::TextStart { index: 1 }),
                    Ok(StreamEvent::TextDelta {
                        index: 1,
                        text: "final".into(),
                    }),
                    Ok(StreamEvent::MessageDone {
                        response: ModelResponse {
                            id: "resp-3".into(),
                            content: vec![ResponseContent::Text("final".into())],
                            stop_reason: Some(StopReason::EndTurn),
                            usage: Usage::default(),
                            metadata: ResponseMetadata {
                                extras: vec![ResponseExtra::ReasoningText {
                                    text: "plan".into(),
                                }],
                            },
                        },
                    }),
                ])))
            }

            fn name(&self) -> &str {
                "reasoning-provider"
            }
        }

        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("hello"));
        let seen_events = Arc::new(Mutex::new(Vec::new()));
        let callback_events = Arc::clone(&seen_events);
        let callback = Arc::new(move |event: QueryEvent| {
            callback_events.lock().expect("lock callback").push(event);
        });

        query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            Arc::new(ReasoningProvider),
            registry,
            &runtime,
            Some(callback),
        )
        .await
        .expect("query should succeed");

        let events = seen_events.lock().expect("lock events");
        assert!(events.iter().any(|event| matches!(
            event,
            QueryEvent::ReasoningDelta(text) if text == "plan"
        )));
        drop(events);

        let assistant_message = session
            .messages
            .iter()
            .find(|message| matches!(message.role, Role::Assistant))
            .expect("assistant message");
        assert_eq!(
            assistant_message,
            &Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::Reasoning {
                        text: "plan".into(),
                    },
                    ContentBlock::Text {
                        text: "final".into(),
                    },
                ],
            }
        );
    }

    #[tokio::test]
    async fn query_disables_openai_thinking_when_reasoning_context_is_missing() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(OpenAiCapturingProvider {
            requests: Arc::clone(&requests),
        });
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let model = Model {
            slug: "deepseek-v4-flash".into(),
            provider: devo_protocol::ProviderWireApi::OpenAIChatCompletions,
            thinking_capability: ThinkingCapability::Toggle,
            base_instructions: String::new(),
            ..Model::default()
        };
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::assistant_text("legacy assistant reply"));
        session.push_message(Message::user("follow up"));

        query(
            &mut session,
            &TurnConfig::new(model, Some("enabled".into())),
            Arc::clone(&provider),
            registry,
            &runtime,
            None,
        )
        .await
        .expect("query should succeed");

        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].thinking.as_deref(), Some("enabled"));
        // Toggle capability does not set reasoning_effort on the request.
        assert_eq!(captured[0].reasoning_effort, None);
    }

    #[tokio::test]
    async fn query_tool_result_summary_is_set() {
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler("mutating_tool", Arc::new(MutatingTool));
        builder.push_spec(ToolSpec {
            name: "mutating_tool".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::Mutating,
            capability_tags: vec![],
            supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = Arc::new(builder.build());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));

        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("run the tool"));

        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_clone = Arc::clone(&seen);
        let callback = Arc::new(move |event: QueryEvent| {
            if let QueryEvent::ToolResult { summary, .. } = event {
                seen_clone.lock().unwrap().push(summary);
            }
        });

        query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            Arc::new(SingleToolUseProvider {
                requests: AtomicUsize::new(0),
            }),
            registry,
            &runtime,
            Some(callback),
        )
        .await
        .expect("query should complete");

        let summaries = seen.lock().unwrap();
        assert!(
            !summaries.is_empty(),
            "should have at least one ToolResult summary"
        );
        for summary in summaries.iter() {
            assert!(!summary.is_empty(), "summary should not be empty");
        }
    }

    #[tokio::test]
    async fn query_tool_result_event_includes_final_tool_input() {
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler("mutating_tool", Arc::new(DisplayContentTool));
        builder.push_spec(ToolSpec {
            name: "mutating_tool".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = Arc::new(builder.build());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));

        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("run the tool"));

        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_clone = Arc::clone(&seen);
        let callback = Arc::new(move |event: QueryEvent| {
            if let QueryEvent::ToolResult {
                tool_name, input, ..
            } = event
            {
                seen_clone.lock().unwrap().push((tool_name, input));
            }
        });

        query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            Arc::new(SingleToolUseProvider {
                requests: AtomicUsize::new(0),
            }),
            registry,
            &runtime,
            Some(callback),
        )
        .await
        .expect("query should complete");

        assert_eq!(
            seen.lock().unwrap().as_slice(),
            &[(String::from("mutating_tool"), json!({ "value": 1 }))]
        );
    }

    #[tokio::test]
    async fn query_tool_result_event_matches_input_delta_by_tool_index() {
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler("mutating_tool", Arc::new(DisplayContentTool));
        builder.push_spec(ToolSpec {
            name: "mutating_tool".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = Arc::new(builder.build());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));

        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("run the tools"));

        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_clone = Arc::clone(&seen);
        let callback = Arc::new(move |event: QueryEvent| {
            if let QueryEvent::ToolResult {
                tool_use_id, input, ..
            } = event
            {
                seen_clone.lock().unwrap().push((tool_use_id, input));
            }
        });

        query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            Arc::new(InterleavedToolUseProvider {
                requests: AtomicUsize::new(0),
            }),
            registry,
            &runtime,
            Some(callback),
        )
        .await
        .expect("query should complete");

        assert_eq!(
            seen.lock().unwrap().as_slice(),
            &[
                (String::from("tool-1"), json!({ "value": 1 })),
                (String::from("tool-2"), json!({ "value": 2 })),
            ]
        );
    }

    #[tokio::test]
    async fn query_tool_start_event_includes_final_tool_input() {
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler("mutating_tool", Arc::new(DisplayContentTool));
        builder.push_spec(ToolSpec {
            name: "mutating_tool".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = Arc::new(builder.build());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));

        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("run the tools"));

        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_clone = Arc::clone(&seen);
        let callback = Arc::new(move |event: QueryEvent| {
            if let QueryEvent::ToolUseStart { id, input, .. } = event {
                seen_clone.lock().unwrap().push((id, input));
            }
        });

        query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            Arc::new(InterleavedToolUseProvider {
                requests: AtomicUsize::new(0),
            }),
            registry,
            &runtime,
            Some(callback),
        )
        .await
        .expect("query should complete");

        assert_eq!(
            seen.lock().unwrap().as_slice(),
            &[
                (String::from("tool-1"), json!({ "value": 1 })),
                (String::from("tool-2"), json!({ "value": 2 })),
            ]
        );
    }

    #[tokio::test]
    #[ignore = "legacy progress mechanism replaced by L3 contracts"]
    async fn query_emits_tool_result_display_content() {
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler("mutating_tool", Arc::new(DisplayContentTool));
        builder.push_spec(ToolSpec {
            name: "mutating_tool".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = Arc::new(builder.build());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));

        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("run the tool"));

        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_clone = Arc::clone(&seen);
        let callback = Arc::new(move |event: QueryEvent| {
            if let QueryEvent::ToolResult {
                content,
                display_content,
                ..
            } = event
            {
                seen_clone.lock().unwrap().push((content, display_content));
            }
        });

        query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            Arc::new(SingleToolUseProvider {
                requests: AtomicUsize::new(0),
            }),
            registry,
            &runtime,
            Some(callback),
        )
        .await
        .expect("query should complete");

        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert!(matches!(
            &seen[0],
            (crate::tools::ToolContent::Text(text), Some(display))
                if text == "canonical" && display == "display"
        ));
    }

    #[tokio::test]
    #[ignore = "legacy progress mechanism replaced by L3 contracts"]
    async fn query_emits_tool_progress_before_tool_result() {
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler("mutating_tool", Arc::new(StreamingMutatingTool));
        builder.push_spec(ToolSpec {
            name: "mutating_tool".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::Mutating,
            capability_tags: vec![],
            supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = Arc::new(builder.build());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));

        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("run the tool"));

        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_clone = Arc::clone(&seen);
        let callback = Arc::new(move |event: QueryEvent| {
            seen_clone.lock().unwrap().push(event);
        });

        query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            Arc::new(SingleToolUseProvider {
                requests: AtomicUsize::new(0),
            }),
            registry,
            &runtime,
            Some(callback),
        )
        .await
        .expect("query should complete");

        let events = seen.lock().unwrap();
        let progress_index = events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    QueryEvent::ToolProgress {
                        tool_use_id,
                        content,
                    } if tool_use_id == "tool-1" && content == "stream chunk\n"
                )
            })
            .expect("tool progress event should be emitted");
        let result_index = events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    QueryEvent::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                        ..
                    } if tool_use_id == "tool-1"
                        && matches!(content, crate::tools::ToolContent::Text(text) if text == "stream complete")
                        && !is_error
                )
            })
            .expect("tool result event should be emitted");

        assert!(
            progress_index < result_index,
            "tool progress should arrive before final result"
        );
    }

    #[tokio::test]
    async fn query_emits_parallel_tool_results_as_each_tool_finishes() {
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler("parallel_tool", Arc::new(ParallelDelayTool));
        builder.push_spec(ToolSpec {
            name: "parallel_tool".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: true,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = Arc::new(builder.build());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));

        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("run the tools"));

        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_clone = Arc::clone(&seen);
        let callback = Arc::new(move |event: QueryEvent| match event {
            QueryEvent::ToolUseStart { id, .. } => {
                seen_clone
                    .lock()
                    .expect("lock events")
                    .push(format!("start:{id}"));
            }
            QueryEvent::ToolResult {
                tool_use_id,
                content,
                ..
            } => {
                let content = content.into_string();
                seen_clone
                    .lock()
                    .expect("lock events")
                    .push(format!("result:{tool_use_id}:{content}"));
            }
            _ => {}
        });

        query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            Arc::new(ParallelToolUseProvider {
                requests: AtomicUsize::new(0),
            }),
            registry,
            &runtime,
            Some(callback),
        )
        .await
        .expect("query should complete");

        assert_eq!(
            seen.lock().expect("lock events").as_slice(),
            &[
                "start:slow".to_string(),
                "start:fast".to_string(),
                "result:fast:fast complete".to_string(),
                "result:slow:slow complete".to_string(),
            ]
        );

        let tool_result_ids = session
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .filter_map(|block| match block {
                ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(tool_result_ids, vec!["slow", "fast"]);
    }
}

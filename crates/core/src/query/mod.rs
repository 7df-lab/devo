//! Agent query loop: stream a model turn, run tools, and continue.
//!
//! Submodules isolate the observation surface, provider retry policy, prompt
//! token estimates, stream consumption, and model-turn continuation quirks
//! from the orchestration loop in this file.

mod context_recovery;
mod event;
mod kernel_brief;
mod pending_input;
mod prompt_estimate;
mod provider_retry;
mod stream_consumer;
mod turn_continuation;

pub use devo_protocol::native::event::ModelQueryRetryPhase;
pub use event::EventCallback;
pub use event::LiveTurnSettings;
pub use event::ProviderRetryStatus;
pub use event::QueryEvent;
pub use event::QueryOptions;
pub use event::SharedLastModelRequest;
pub use event::SharedLiveGoal;
pub use event::SharedLiveTurnSettings;

pub(crate) use event::emit_query_event;
pub use prompt_estimate::RawContextBreakdown;
pub(crate) use prompt_estimate::estimate_request_context_breakdown;
pub(crate) use provider_retry::ProviderRetryDecision;
pub(crate) use provider_retry::provider_retry_decision;
pub(crate) use provider_retry::wait_for_provider_retry;
pub(crate) use stream_consumer::AssembledModelTurn;
pub(crate) use stream_consumer::ProviderAttemptError;
pub(crate) use stream_consumer::run_provider_attempt;
pub(crate) use turn_continuation::ModelTurnSnapshot;
pub(crate) use turn_continuation::TurnContinuation;
pub(crate) use turn_continuation::TurnContinuationPolicy;
pub(crate) use turn_continuation::assistant_content_has_visible_content;

pub(crate) use provider_retry::ErrorClass;
pub(crate) use provider_retry::classify_error;
#[cfg(test)]
pub(crate) use turn_continuation::DEEPSEEK_THINKING_ONLY_CONTINUATION_PROMPT;

use std::collections::HashMap;
use std::sync::Arc;

use devo_protocol::HostedToolDefinition;
use devo_protocol::HostedWebFetchTool;
use devo_protocol::HostedWebSearchTool;
use devo_protocol::ModelRequest;
use devo_protocol::ProviderWireApi;
use devo_protocol::RequestContent;
use devo_protocol::RequestMessage;
use devo_protocol::ResolvedReasoningRequest;
use devo_protocol::ResponseContent;
use devo_protocol::SamplingControls;
use devo_protocol::StreamEvent;
use devo_protocol::TruncationPolicy;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::debug;
use tracing::info;
use tracing::info_span;
use tracing::warn;

use crate::tools::ToolAgentScope;
use crate::tools::ToolContent;
use crate::tools::ToolRegistry;
use crate::tools::ToolRuntime;
use devo_provider::ModelProviderSDK;

use self::context_recovery::CompactionModelRequest;
use self::context_recovery::OVERFLOW_COMPACTION_KINDS;
use self::context_recovery::summarize_and_compact;

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
use crate::history::compaction::prefix::AcceptedPrefix;
use crate::response_item::ResponseItem;

const SUBAGENT_MODE_REMINDER: &str = include_str!("../../prompts/subagent_mode_reminder.md");

fn supports_provider_hosted_web_tools(wire_api: ProviderWireApi) -> bool {
    matches!(
        wire_api,
        ProviderWireApi::AnthropicMessages | ProviderWireApi::OpenAIResponses
    )
}

fn provider_route_wire_api(route: &devo_provider::ProviderRoute) -> Option<ProviderWireApi> {
    match route {
        devo_provider::ProviderRoute::Default => None,
        devo_provider::ProviderRoute::Connection { wire_api, .. } => Some(*wire_api),
    }
}

fn hosted_tools_for_web_capabilities(
    web_search: &devo_config::ResolvedWebSearchConfig,
    web_fetch: &devo_config::ResolvedWebFetchConfig,
    wire_api: ProviderWireApi,
) -> Vec<HostedToolDefinition> {
    let mut hosted_tools = Vec::new();
    let provider_search = matches!(web_search, devo_config::ResolvedWebSearchConfig::Provider)
        && supports_provider_hosted_web_tools(wire_api);
    let fetch_via_search =
        web_fetch.is_provider_via_search() && wire_api == ProviderWireApi::OpenAIResponses;
    if provider_search || fetch_via_search {
        hosted_tools.push(HostedToolDefinition::WebSearch(HostedWebSearchTool::new()));
    }
    if web_fetch.is_provider() && supports_provider_hosted_web_tools(wire_api) {
        hosted_tools.push(HostedToolDefinition::WebFetch(HostedWebFetchTool::new()));
    }
    hosted_tools
}

#[cfg(test)]
fn hosted_tools_for_web_search(
    web_search: &devo_config::ResolvedWebSearchConfig,
) -> Vec<HostedToolDefinition> {
    hosted_tools_for_web_capabilities(
        web_search,
        &devo_config::ResolvedWebFetchConfig::Disabled,
        ProviderWireApi::AnthropicMessages,
    )
}

// ---------------------------------------------------------------------------
// Model-visible tool result truncation
// ---------------------------------------------------------------------------

const TOOL_RESULT_TRUNCATION_MARKER: &str = "\n...[truncated]";

/// Tools that store the model-facing payload in Mixed `text` and put UI/protocol
/// metadata in `json` (read preview/truncated). Omit JSON from the prompt so the
/// stream is not duplicated. Shell results are NOT omitted: their `json` holds
/// no copy of the output stream and carries `duration_ms`/exit/cwd, which the
/// model needs to reason about slow, hung, or interrupted commands.
fn tool_result_omits_mixed_json_for_model(tool_name: Option<&str>) -> bool {
    matches!(tool_name, Some("read"))
}

fn serialize_tool_content_for_model(content: ToolContent, tool_name: Option<&str>) -> String {
    if tool_result_omits_mixed_json_for_model(tool_name) {
        content.text_for_model()
    } else {
        content.into_string()
    }
}

fn tool_content_model_bytes(content: &ToolContent, tool_name: Option<&str>) -> usize {
    if tool_result_omits_mixed_json_for_model(tool_name) {
        content.text_for_model_byte_len()
    } else {
        content.into_string_byte_len()
    }
}

fn truncate_tool_result_for_model(
    content: String,
    tool_name: Option<&str>,
    truncation_policy: TruncationPolicy,
) -> String {
    if preserve_full_tool_result(tool_name) {
        return content;
    }

    let byte_budget = truncation_policy.byte_budget();
    if content.len() <= byte_budget {
        return content;
    }

    let marker = if byte_budget > TOOL_RESULT_TRUNCATION_MARKER.len() {
        TOOL_RESULT_TRUNCATION_MARKER
    } else {
        TOOL_RESULT_TRUNCATION_MARKER.trim_start()
    };

    if byte_budget <= marker.len() {
        return marker.to_string();
    }

    let content_budget = byte_budget - marker.len();
    let mut truncate_at = content_budget;
    while truncate_at > 0 && !content.is_char_boundary(truncate_at) {
        truncate_at -= 1;
    }

    let mut truncated = content[..truncate_at].to_string();
    truncated.push_str(marker);
    truncated
}

fn preserve_full_tool_result(tool_name: Option<&str>) -> bool {
    matches!(
        tool_name,
        Some("await_task" | "wait_agent" | "subagent_result")
    )
}

fn insert_subagent_request_reminders(messages: &mut Vec<RequestMessage>) {
    let insert_at = messages
        .iter()
        .rposition(is_user_text_message)
        .unwrap_or(messages.len());
    messages.splice(
        insert_at..insert_at,
        [request_text_message(
            SUBAGENT_MODE_REMINDER.trim_end().to_string(),
        )],
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

/// Inject Continual Harness digest after skills prefix, before hidden goal.
fn insert_harness_digest_message(messages: &mut Vec<RequestMessage>, digest: &str) {
    let insert_at = if messages.last().is_some_and(is_visible_user_text_message) {
        messages.len().saturating_sub(1)
    } else {
        messages.len()
    };
    messages.splice(
        insert_at..insert_at,
        [request_text_message(digest.to_string())],
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
                    || trimmed.starts_with("<live_python_names>")
                    || trimmed.starts_with("<background_work_results>")
                    || trimmed.starts_with("<available_skills>")
                    || trimmed.starts_with("<language_preference>")
                    || trimmed.starts_with("<context_changes>")
                    || trimmed.starts_with("<user_instructions_updates>")
                    || trimmed.starts_with("<user_instructions>")
                    || trimmed.contains("[harness-digest]")
                    || trimmed.starts_with("## Continual harness")
            }
            RequestContent::Reasoning { .. }
            | RequestContent::ProviderReasoning { .. }
            | RequestContent::HostedToolUse { .. }
            | RequestContent::ToolUse { .. }
            | RequestContent::ToolResult { .. }
            | RequestContent::Image { .. } => false,
        })
}

/// Agent loop orchestration: build request, stream, continue or run tools.
///
/// Observation (`event`), provider retry (`provider_retry`), prompt estimates
/// (`prompt_estimate`), stream consumption (`stream_consumer`), and model-turn
/// continuation quirks (`turn_continuation`) live in submodules.
///
/// The recursive agent loop is the beating heart of the runtime.
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
    options: QueryOptions,
) -> Result<(), AgentError> {
    let mut turn_user_inputs: Vec<_> = options
        .turn_user_inputs
        .into_iter()
        .flat_map(crate::response_item::message_to_response_items)
        .collect();
    if let Some(journal) = &options.journal {
        let replay = journal.replay().await.map_err(AgentError::Provider)?;
        if let Some(counters) = &replay.counters {
            counters.restore(session);
        }
        if replay.completed {
            session.set_prompt_messages(crate::history::response_items_to_messages(&replay.items));
            if let Some(stop_reason) = replay.stop_reason {
                emit_query_event(&on_event, QueryEvent::TurnComplete { stop_reason }).await;
            }
            session.end_turn();
            return Ok(());
        }
        if !replay.turn_input_indices.is_empty() {
            turn_user_inputs = replay
                .turn_input_indices
                .iter()
                .map(|&index| {
                    replay.items.get(index).cloned().ok_or_else(|| {
                        AgentError::Provider(anyhow::anyhow!(
                            "invalid user input position {index} in prompt checkpoint"
                        ))
                    })
                })
                .collect::<Result<_, _>>()?;
        }
    }
    if turn_user_inputs.is_empty() {
        // Standalone callers can enter the loop with several user parts followed by tool results.
        let messages = session.prompt_source_messages();
        let is_human = crate::history::compaction::is_user_message;
        if let Some(last) = messages.iter().rposition(is_human) {
            let first = messages[..last]
                .iter()
                .rposition(|msg| !is_human(msg))
                .map_or(0, |i| i + 1);
            turn_user_inputs.extend(
                messages[first..=last]
                    .iter()
                    .cloned()
                    .flat_map(crate::response_item::message_to_response_items),
            );
        }
    }
    let compaction_provider = options
        .compaction_provider
        .as_ref()
        .unwrap_or(&provider)
        .clone();
    let same_compaction_backend = Arc::ptr_eq(
        provider.prompt_cache_backend().unwrap_or(&provider),
        compaction_provider
            .prompt_cache_backend()
            .unwrap_or(&compaction_provider),
    );
    let restricted_runtime = registry.model_tool_allowlist().map(|names| {
        let names = names.iter().map(String::as_str).collect::<Vec<_>>();
        runtime.restricted_to_specs(&names)
    });
    let model_runtime = restricted_runtime.as_ref().unwrap_or(runtime);
    let agents_md_manager = AgentsMdManager::new(session.config.agents_md.clone());
    let current_agents_snapshot = load_workspace_instructions(&session.cwd, &agents_md_manager);
    let agent_scope = runtime.agent_scope();

    if session.session_context.is_none() {
        session.session_context = Some(SessionContext::capture(
            &turn_config.model,
            turn_config.reasoning_effort_selection.as_deref(),
            &session.cwd,
            current_agents_snapshot.clone(),
            session.config.available_skills_instructions.clone(),
        ));
    }
    // Skills are live catalog state, unlike the locked workspace/model snapshot.
    if let Some(context) = session.session_context.as_mut() {
        context.available_skills = session.config.available_skills_instructions.clone();
        context.installed_python_skills = session.config.installed_python_skills.clone();
    }
    let current_turn_context =
        TurnContext::capture(session, turn_config, current_agents_snapshot.clone());
    if let Some(context_changes) =
        current_turn_context.context_changes_since(session.latest_turn_context.as_ref())
    {
        session.insert_context_message(context_changes.to_message());
    }
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
    let mut locked_messages = Vec::new();
    crate::history::prepend_user_inputs(&mut locked_messages, &prefetched_user_inputs);
    let mut accepted_prefix = None;

    let mut retry_count: usize = 0;
    let mut pending_usage = Some(session.last_turn_tokens);
    let mut overflow_compactions = OVERFLOW_COMPACTION_KINDS.into_iter();
    let mut budget_steer_injected = false;
    // Loop guard: a model emitting the same tool batch over and over
    // (observed in the wild: 100+ identical ToolSearch calls in one turn)
    // burns provider calls indefinitely. Count consecutive identical batches
    // and inject a visible steering note every IDENTICAL_BATCH_NOTICE_INTERVAL
    // of them so the loop breaks without hard-aborting legitimate retries.
    let mut last_batch_signature: Option<u64> = None;
    let mut identical_batch_streak: u32 = 0;
    const IDENTICAL_BATCH_NOTICE_INTERVAL: u32 = 5;
    let mut continuation_policy =
        TurnContinuationPolicy::for_models(&turn_config.model.slug, &turn_config.request_model);
    // Live settings override (L2-DES-CONV-002 Phase 4): the active config
    // starts as the turn-start snapshot and is re-applied per iteration when
    // the shared generation advances.
    let mut active_turn_config = turn_config.clone();
    let mut applied_live_generation = 0u64;

    if session.turn_state.is_none() {
        session.start_turn(devo_protocol::TurnKind::Regular);
    }

    // Explicit interrupted-turn notice for the next user message after a user
    // interrupt. Placed before pending-input processing so it sits just above the
    // latest user text in prompt construction (after any context diff).
    let previous_turn_interrupted = session.take_last_turn_interrupted();
    if previous_turn_interrupted {
        let fragment = TurnAborted::new(TurnAborted::INTERRUPTED_GUIDANCE);
        if let ResponseItem::Message(msg) = fragment.to_response_item() {
            session.insert_context_message(msg);
        }
    }

    loop {
        if let Some(boundary) = &options.inbox_boundary {
            boundary().await.map_err(AgentError::Provider)?;
        }
        let pending = session.take_turn_pending_input();

        turn_user_inputs.extend(pending_input::push_pending_input(session, &pending));

        // Host goal changes must reach the checked-out working copy before
        // assembling another request, including after a Python completion call.
        if let Some(live_goal) = &options.live_goal {
            let update = live_goal.lock().expect("live goal mutex poisoned").clone();
            if let Some(goal) = update {
                match goal {
                    Some(goal) => session.set_active_goal(goal),
                    None => session.clear_active_goal(),
                }
            }
        }
        // Check token budget and compact before building the request
        if let Some(live) = &options.live_settings {
            let live = live.lock().expect("live settings mutex poisoned");
            if live.generation != applied_live_generation {
                if let Some(config) = &live.turn_config {
                    accepted_prefix = None;
                    active_turn_config = config.clone();
                }
                if let Some(limit) = live.auto_compact_token_limit {
                    session.config.token_budget.context_window = limit;
                    session.config.token_budget.auto_compact_token_limit = Some(limit);
                }
                applied_live_generation = live.generation;
            }
        }
        session.turn_count += 1;
        let turn_span = info_span!(
            "turn",
            turn = session.turn_count,
            session_id = %session.id,
            model = %active_turn_config.model.slug,
            cwd = %session.cwd.display()
        );
        let _turn_guard = turn_span.enter();
        info!("starting turn");

        // Build system instructions from session-locked context plus the active mode.
        let request_system = {
            let mut system = session_context
                .build_system_prompt_for_agent(session.collaboration_mode, &options.agent_identity);
            if turn_config.web_search.is_provider() {
                if !system.trim().is_empty() {
                    system.push_str("\n\n");
                }
                system.push_str(&crate::tools::websearch_prompt::web_search_prompt());
            } else if matches!(
                turn_config.web_search,
                devo_config::ResolvedWebSearchConfig::Local(_)
                    | devo_config::ResolvedWebSearchConfig::Mcp { .. }
            ) {
                if !system.trim().is_empty() {
                    system.push_str("\n\n");
                }
                system.push_str(
                    "Configured web search is available through Python as `await rlm.web_search(query)`. Use it for current information when needed.",
                );
            }
            if turn_config.web_fetch.is_provider_via_search()
                && provider_route_wire_api(&turn_config.provider_route)
                    == Some(ProviderWireApi::OpenAIResponses)
            {
                if !system.trim().is_empty() {
                    system.push_str("\n\n");
                }
                system.push_str("To read a specific URL, use the OpenAI Responses hosted web_search `open_page` action. Do not use Python, direct HTTP, or another fetch backend. Keep the configured provider and supplier unchanged.");
            } else if turn_config.web_fetch.is_local() {
                if !system.trim().is_empty() {
                    system.push_str("\n\n");
                }
                system.push_str(
                    "Local web fetch is available through Python as `await rlm.web_fetch(url)`. Use it to retrieve a specific URL when needed.",
                );
            } else if turn_config.web_fetch.is_provider()
                && provider_route_wire_api(&turn_config.provider_route)
                    .is_none_or(supports_provider_hosted_web_tools)
            {
                if !system.trim().is_empty() {
                    system.push_str("\n\n");
                }
                system.push_str("Provider-hosted web fetch is available to retrieve a specific URL when needed.");
            } else if matches!(
                &turn_config.web_fetch,
                devo_config::ResolvedWebFetchConfig::CustomMcp { .. }
            ) {
                if !system.trim().is_empty() {
                    system.push_str("\n\n");
                }
                system.push_str(
                    "The selected MCP search server may also provide a compatible web_fetch tool through `await rlm.web_fetch(url)`. Use it only when available; otherwise continue with search and do not fall back to another fetch backend.",
                );
            } else if matches!(
                &turn_config.web_fetch,
                devo_config::ResolvedWebFetchConfig::Mcp { .. }
            ) {
                if !system.trim().is_empty() {
                    system.push_str("\n\n");
                }
                system.push_str(
                    "Configured MCP web fetch is available through Python as `await rlm.web_fetch(url)`. Use it to retrieve a specific URL when needed.",
                );
            }
            Some(system).filter(|system| !system.trim().is_empty())
        };

        // Resolve provider-bound reasoning request parameters from the live
        // config so mid-turn model/effort changes apply at this model call.
        let ResolvedReasoningRequest {
            request_model,
            request_thinking,
            request_reasoning_effort,
            extra_body,
            effective_reasoning_effort: _,
        } = active_turn_config.model.resolve_reasoning_effort_selection(
            active_turn_config.reasoning_effort_selection.as_deref(),
        );
        let catalog_request_model = request_model.clone();
        let provider_request_model =
            active_turn_config.provider_request_model(&catalog_request_model);
        let extra_body = crate::merge_model_request_body(
            active_turn_config
                .provider_request_models
                .request_defaults(),
            extra_body,
        );
        let extra_body = crate::add_model_request_headers(
            extra_body,
            active_turn_config.provider_request_models.request_headers(),
        );

        let prompt_source_message_count = session.prompt_source_messages().len();
        let history_items = session.prompt_items();
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
        let mut messages = history.for_prompt(&active_turn_config.model.input_modalities);
        messages.splice(..0, locked_messages.iter().cloned());
        // Internal registries may retain handlers for server-side operations, but
        // only the Python kernel schema is ever sent to a model provider.
        let model_registry = registry.restricted_to_specs(&["ipython"]);
        let request_tools = model_registry.tool_definitions();
        // Continual Harness digest: after skills (prefix), before hidden goal.
        if let Some(digest) = options
            .harness_digest
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
        {
            insert_harness_digest_message(&mut messages, digest);
        }
        if let Some(results) = options.background_work_results.as_deref() {
            insert_goal_context_message(&mut messages, results);
        }
        if let Some(goal_context) = session.goal_context_prompt() {
            insert_goal_context_message(&mut messages, &goal_context);
        }
        if agent_scope == ToolAgentScope::Subagent {
            insert_subagent_request_reminders(&mut messages);
        }

        let hosted_tools = hosted_tools_for_web_capabilities(
            &turn_config.web_search,
            &turn_config.web_fetch,
            active_turn_config.model.provider,
        );
        let mut request = ModelRequest {
            model_slug: devo_protocol::ModelProfileKey::CatalogSlug(catalog_request_model.clone()),
            model: provider_request_model,
            system: request_system,
            messages,
            max_tokens: active_turn_config
                .model
                .max_tokens
                .map_or(session.config.token_budget.max_output_tokens, |value| {
                    value as usize
                }),
            tools: (!request_tools.is_empty()).then(|| request_tools.clone()),
            hosted_tools: hosted_tools.clone(),
            sampling: SamplingControls {
                temperature: active_turn_config.model.temperature,
                top_p: active_turn_config.model.top_p,
                top_k: active_turn_config.model.top_k.map(|value| value as u32),
            },
            request_thinking,
            reasoning_effort: request_reasoning_effort,
            extra_body,
        };
        // Auto-review and cell-watch reuse this prefix after Python may have
        // changed state. Keep the live observation exclusive to this attempt.
        let reusable_request = options.last_model_request.as_ref().map(|_| request.clone());
        kernel_brief::insert_live_brief(
            &history,
            runtime.context().kernel.as_deref(),
            &mut request.messages,
        )
        .await;
        let breakdown = estimate_request_context_breakdown(&request);
        if !overflow_compactions.as_slice().is_empty() {
            let budget = &session.config.token_budget;
            let estimated_input = usize::try_from(breakdown.total()).unwrap_or(usize::MAX);
            let wire_api = provider_route_wire_api(&active_turn_config.provider_route)
                .unwrap_or(active_turn_config.model.provider);
            let output_reserve = context_recovery::requested_output_tokens(&request, wire_api);
            // Consume each provider sample once, even if consecutive calls report the same total.
            let usage_pressure = pending_usage
                .take()
                .is_some_and(|tokens| budget.should_compact(tokens));
            if usage_pressure
                || budget.should_compact(estimated_input)
                || estimated_input.saturating_add(output_reserve) > budget.context_window
            {
                // Re-estimate each replacement before dispatch. Each pressure
                // level can run only once before the next successful model call.
                let compacted = summarize_and_compact(
                    session,
                    &on_event,
                    CompactionModelRequest {
                        turn_user_inputs: &turn_user_inputs,
                        journal: options.journal.as_deref(),
                        provider: &compaction_provider,
                        accepted_prefix: accepted_prefix.as_ref(),
                        model_slug: &catalog_request_model,
                        request_model: &request.model,
                        max_tokens: request.max_tokens,
                    },
                    overflow_compactions.by_ref(),
                    options.cancel_token.as_ref(),
                )
                .await;
                if options
                    .cancel_token
                    .as_ref()
                    .is_some_and(CancellationToken::is_cancelled)
                {
                    return Err(AgentError::Aborted);
                }
                if compacted {
                    accepted_prefix = None;
                    session.turn_count -= 1;
                    continue;
                }
                if !budget_steer_injected && let Some(turn) = session.turn_state.as_mut() {
                    turn.push_pending_input(devo_protocol::PendingInputItem::new(
                        devo_protocol::PendingInputKind::BudgetLimitSteering,
                        /*metadata*/ None,
                        chrono::Utc::now(),
                    ));
                    budget_steer_injected = true;
                }
            }
        }
        if let (Some(slot), Some(reusable_request)) =
            (&options.last_model_request, reusable_request)
            && let Ok(mut last) = slot.lock()
        {
            *last = Some(reusable_request);
        }
        tracing::debug!(
            tool_count = request_tools.len(),
            names = ?request_tools.iter().map(|t| t.name.clone()).collect::<Vec<_>>(),
            "provider request tools"
        );
        session.prompt_token_estimate = breakdown.total().try_into().unwrap_or(usize::MAX);
        session.raw_context_breakdown = Some(breakdown);
        emit_query_event(&on_event, QueryEvent::ContextEstimate { breakdown }).await;
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

        if let Some(journal) = &options.journal {
            journal
                .commit(context_recovery::prompt_checkpoint(
                    session,
                    history.items,
                    &turn_user_inputs,
                ))
                .await
                .map_err(AgentError::Provider)?;
        }

        let candidate = if same_compaction_backend {
            AcceptedPrefix::capture(&request, &locked_messages)
        } else {
            None
        };
        let assembled = match run_provider_attempt(
            provider.as_ref(),
            request,
            session,
            &on_event,
            options.cancel_token.as_ref(),
            &turn_config.model.slug,
        )
        .await
        {
            Ok(assembled) => {
                accepted_prefix = candidate;
                retry_count = 0;
                overflow_compactions = OVERFLOW_COMPACTION_KINDS.into_iter();
                assembled
            }
            Err(ProviderAttemptError::Fatal(error)) => {
                return Err(AgentError::Provider(error));
            }
            Err(error) => {
                let (is_create, retry_error) = match error {
                    ProviderAttemptError::Create(error) => (true, error),
                    ProviderAttemptError::Retryable(error) => (false, error),
                    ProviderAttemptError::Fatal(_) => {
                        unreachable!("fatal errors handled above")
                    }
                };
                match provider_retry_decision(&retry_error, &mut retry_count) {
                    ProviderRetryDecision::CompactAndRetry => {
                        let retry_compaction_model_slug = active_turn_config
                            .model
                            .resolve_reasoning_effort_selection(
                                active_turn_config.reasoning_effort_selection.as_deref(),
                            )
                            .request_model;
                        let retry_compaction_request_model =
                            active_turn_config.provider_request_model(&retry_compaction_model_slug);
                        let compacted = summarize_and_compact(
                            session,
                            &on_event,
                            CompactionModelRequest {
                                turn_user_inputs: &turn_user_inputs,
                                journal: options.journal.as_deref(),
                                provider: &compaction_provider,
                                accepted_prefix: accepted_prefix.as_ref(),
                                model_slug: &retry_compaction_model_slug,
                                request_model: &retry_compaction_request_model,
                                max_tokens: active_turn_config.model.max_tokens.unwrap_or(4096)
                                    as usize,
                            },
                            overflow_compactions.by_ref(),
                            options.cancel_token.as_ref(),
                        )
                        .await;
                        if options
                            .cancel_token
                            .as_ref()
                            .is_some_and(CancellationToken::is_cancelled)
                        {
                            return Err(AgentError::Aborted);
                        }
                        if !compacted {
                            return Err(AgentError::Provider(retry_error));
                        }
                        accepted_prefix = None;
                        session.turn_count -= 1;
                        continue;
                    }
                    ProviderRetryDecision::RetryAfter(backoff) => {
                        if is_create {
                            warn!(
                                attempt = retry_count,
                                backoff_ms = backoff.as_millis(),
                                "transient provider error - retrying with exponential backoff"
                            );
                        } else {
                            warn!(
                                attempt = retry_count,
                                backoff_ms = backoff.as_millis(),
                                "transient provider stream error - retrying with exponential backoff"
                            );
                        }
                        wait_for_provider_retry(
                            &on_event,
                            options.cancel_token.as_ref(),
                            provider.name(),
                            &turn_config.model.slug,
                            retry_count,
                            backoff,
                            &retry_error.to_string(),
                        )
                        .await?;
                        session.turn_count -= 1;
                        continue;
                    }
                    ProviderRetryDecision::Fail => {
                        if retry_count > 0 {
                            emit_query_event(
                                &on_event,
                                QueryEvent::ProviderQueryFailed {
                                    attempt: retry_count,
                                    max_attempts: provider_retry::max_provider_retries(),
                                    message: retry_error.to_string(),
                                },
                            )
                            .await;
                        }
                        return Err(AgentError::Provider(retry_error));
                    }
                }
            }
        };

        let AssembledModelTurn {
            mut assistant_content,
            tool_calls,
            stop_reason,
            has_hosted_tool_uses,
            has_provider_reasoning,
            has_visible_assistant_text,
            reported_context_tokens,
        } = assembled;
        pending_usage = reported_context_tokens;

        let continuation = continuation_policy.decide(ModelTurnSnapshot {
            stop_reason,
            assistant_content: &assistant_content,
            has_visible_assistant_text,
            has_local_tool_calls: !tool_calls.is_empty(),
            has_hosted_tool_uses,
            has_provider_reasoning,
            request_tools: &request_tools,
            hosted_tools: &hosted_tools,
        });

        if options
            .cancel_token
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            // History may retain partial text, but unfinished arguments and
            // provider reasoning payloads are never valid recovered input.
            assistant_content.retain(|block| matches!(block, ContentBlock::Text { .. }));
        }

        if assistant_content_has_visible_content(&assistant_content) {
            session.push_message(Message {
                role: Role::Assistant,
                content: assistant_content,
            });
        }

        if let Some(journal) = &options.journal {
            journal
                .commit(context_recovery::prompt_checkpoint(
                    session,
                    session.prompt_items(),
                    &turn_user_inputs,
                ))
                .await
                .map_err(AgentError::Provider)?;
        }

        match continuation {
            TurnContinuation::RunTools => {}
            TurnContinuation::Continue => continue,
            TurnContinuation::ContinueWithMessage(message) => {
                session.push_message(message);
                continue;
            }
            TurnContinuation::Complete { stop_reason } => {
                if let Some(boundary) = &options.inbox_boundary
                    && !options
                        .cancel_token
                        .as_ref()
                        .is_some_and(CancellationToken::is_cancelled)
                {
                    boundary().await.map_err(AgentError::Provider)?;
                }
                // Steering can arrive while the final model response streams,
                // even when that response has no tool calls. Consume it at the
                // next request boundary in this turn instead of letting
                // end_turn silently convert accepted steering into a follow-up.
                if !options
                    .cancel_token
                    .as_ref()
                    .is_some_and(CancellationToken::is_cancelled)
                    && !session
                        .steer_input_queue
                        .lock()
                        .expect("steer input queue mutex should not be poisoned")
                        .is_empty()
                {
                    emit_query_event(&on_event, QueryEvent::ModelResponseCompleted).await;
                    continue;
                }
                if let Some(journal) = &options.journal
                    && !options
                        .cancel_token
                        .as_ref()
                        .is_some_and(CancellationToken::is_cancelled)
                {
                    journal
                        .commit(crate::durable_execution::ExecutionRecord::ModelCompleted {
                            items: session.prompt_items(),
                            stop_reason: stop_reason.clone(),
                        })
                        .await
                        .map_err(AgentError::Provider)?;
                }
                if let Some(sr) = stop_reason {
                    emit_query_event(&on_event, QueryEvent::TurnComplete { stop_reason: sr }).await;
                }
                debug!("no tool calls, ending query loop");
                session.end_turn();
                if options
                    .cancel_token
                    .as_ref()
                    .is_some_and(|ct| ct.is_cancelled())
                {
                    return Err(AgentError::Aborted);
                }
                return Ok(());
            }
            TurnContinuation::Fail(error) => return Err(error),
        }

        // If the turn was cancelled (e.g. mid-stream interrupt with partial
        // tool calls), save whatever partial assistant content was already
        // committed above, skip tool execution, and end the turn.
        if options
            .cancel_token
            .as_ref()
            .is_some_and(|ct| ct.is_cancelled())
        {
            session.end_turn();
            return Ok(());
        }

        if let Some(journal) = &options.journal {
            journal
                .commit(crate::durable_execution::ExecutionRecord::IntentBatch {
                    calls: tool_calls
                        .iter()
                        .map(|call| ResponseItem::ToolCall {
                            id: call.id.clone(),
                            name: call.name.clone(),
                            input: call.input.clone(),
                        })
                        .collect(),
                })
                .await
                .map_err(AgentError::Provider)?;
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
        let journal_error = Arc::new(std::sync::Mutex::new(None));
        let results = if let Some(progress_events) = on_event.clone() {
            let journal = options.journal.clone();
            let completion_error = Arc::clone(&journal_error);
            let completion_events = Arc::clone(&progress_events);
            let metadata = Arc::new(tool_result_metadata.clone());
            model_runtime
                .execute_batch_streaming_with_completion(
                    &tool_calls,
                    move |tool_use_id, progress| {
                        let progress_events = Arc::clone(&progress_events);
                        Box::pin(async move {
                            progress_events(QueryEvent::ToolProgress {
                                tool_use_id,
                                progress,
                            })
                            .await;
                        })
                    },
                    move |result| {
                        let completion_events = Arc::clone(&completion_events);
                        let metadata = Arc::clone(&metadata);
                        let journal = journal.clone();
                        let completion_error = Arc::clone(&completion_error);
                        Box::pin(async move {
                            let (tool_name, input, summary) = metadata
                                .get(result.tool_use_id.as_str())
                                .cloned()
                                .unwrap_or_else(|| {
                                    (String::new(), serde_json::Value::Null, String::new())
                                });
                            if let Some(journal) = journal
                                && let Err(error) = journal
                                    .commit(crate::durable_execution::ExecutionRecord::Outcomes {
                                        results: vec![ResponseItem::ToolCallOutput {
                                            tool_use_id: result.tool_use_id.clone(),
                                            content: serialize_tool_content_for_model(
                                                result.content.clone(),
                                                Some(&tool_name),
                                            ),
                                            is_error: result.is_error,
                                        }],
                                    })
                                    .await
                            {
                                *completion_error.lock().expect("journal error lock") = Some(error);
                                return;
                            }
                            completion_events(QueryEvent::ToolResult {
                                tool_use_id: result.tool_use_id,
                                tool_name,
                                input,
                                content: result.content,
                                display_content: result.display_content,
                                is_error: result.is_error,
                                summary,
                            })
                            .await;
                        })
                    },
                )
                .await
        } else {
            model_runtime.execute_batch(&tool_calls).await
        };
        if let Some(error) = journal_error.lock().expect("journal error lock").take() {
            return Err(AgentError::Provider(error));
        }
        if let Some(journal) = &options.journal {
            journal
                .commit(crate::durable_execution::ExecutionRecord::Outcomes {
                    results: results
                        .iter()
                        .map(|result| ResponseItem::ToolCallOutput {
                            tool_use_id: result.tool_use_id.clone(),
                            content: serialize_tool_content_for_model(
                                result.content.clone(),
                                tool_result_metadata
                                    .get(&result.tool_use_id)
                                    .map(|(name, _, _)| name.as_str()),
                            ),
                            is_error: result.is_error,
                        })
                        .collect(),
                })
                .await
                .map_err(AgentError::Provider)?;
        }
        let tool_result_count = results.len();
        let tool_error_count = results.iter().filter(|result| result.is_error).count();
        let tool_output_bytes = results
            .iter()
            .map(|result| {
                let tool_name = tool_result_metadata
                    .get(result.tool_use_id.as_str())
                    .map(|(tool_name, _, _)| tool_name.as_str());
                tool_content_model_bytes(&result.content, tool_name)
            })
            .sum::<usize>();
        debug!(
            tool_calls = tool_calls.len(),
            tool_results = tool_result_count,
            tool_errors = tool_error_count,
            tool_output_bytes,
            "tool batch completed"
        );

        // Build tool result message (user role, per Anthropic API convention).
        // Vision attachments from tools (e.g. attach-image via ipython) are
        // sibling Image blocks so providers receive real multimodal input.
        let truncation_policy = TruncationPolicy::from(turn_config.model.truncation_policy);
        let mut artifacts = Vec::new();
        let result_content: Vec<ContentBlock> = results
            .into_iter()
            .flat_map(|r| {
                artifacts.extend(r.output_artifacts.clone());
                let tool_name = tool_result_metadata
                    .get(r.tool_use_id.as_str())
                    .map(|(tool_name, _, _)| tool_name.as_str());
                let content_str = serialize_tool_content_for_model(r.content, tool_name);
                let content = if r.output_kind == crate::tools::ToolOutputKind::ArtifactPage {
                    content_str
                } else if !r.output_artifacts.is_empty() {
                    let notice = r.output_artifacts.iter().map(devo_tools::output_store::OutputArtifact::notice).collect::<Vec<_>>().join("\n");
                    let budget = truncation_policy.byte_budget().saturating_sub(notice.len() + 1);
                    let mut end = budget.min(content_str.len());
                    while !content_str.is_char_boundary(end) { end -= 1; }
                    format!("{}\n{notice}", &content_str[..end])
                } else if !preserve_full_tool_result(tool_name)
                    && content_str.len() > truncation_policy.byte_budget()
                    && let Some(store) = &options.output_store
                {
                    let notice = match store.save(&r.tool_use_id, content_str.as_bytes()) {
                        Ok(artifact) => { let notice = artifact.notice(); artifacts.push(artifact); notice },
                        Err(error) => format!("Output exceeded the inline limit. Full output could not be saved: {error}"),
                    };
                    let budget = truncation_policy.byte_budget().saturating_sub(notice.len() + 1);
                    let mut end = budget.min(content_str.len());
                    while !content_str.is_char_boundary(end) { end -= 1; }
                    format!("{}\n{notice}", &content_str[..end])
                } else {
                    truncate_tool_result_for_model(content_str, tool_name, truncation_policy)
                };
                let mut blocks = vec![ContentBlock::ToolResult {
                    tool_use_id: r.tool_use_id,
                    content,
                    is_error: r.is_error,
                }];
                for image in r.images {
                    blocks.push(ContentBlock::Image {
                        mime_type: image.mime_type,
                        data_base64: image.data_base64,
                    });
                }
                blocks
            })
            .collect();

        if let Some(journal) = &options.journal {
            journal
                .commit(crate::durable_execution::ExecutionRecord::OutputArtifacts { artifacts })
                .await
                .map_err(AgentError::Provider)?;
        }
        session.push_message(Message {
            role: Role::User,
            content: result_content,
        });
        // Loop guard (see declaration site): nudges the model when the same
        // batch repeats. Placed after the tool-result message so the note sits
        // above the results in the next request.
        {
            use std::hash::Hasher;
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            for call in &tool_calls {
                std::hash::Hash::hash(&call.name, &mut hasher);
                std::hash::Hash::hash(&call.input.to_string(), &mut hasher);
            }
            let batch_signature = hasher.finish();
            identical_batch_streak = if Some(batch_signature) == last_batch_signature {
                identical_batch_streak.saturating_add(1)
            } else {
                1
            };
            last_batch_signature = Some(batch_signature);
            if identical_batch_streak > 0
                && identical_batch_streak.is_multiple_of(IDENTICAL_BATCH_NOTICE_INTERVAL)
            {
                warn!(
                    streak = identical_batch_streak,
                    first_tool = tool_calls.first().map(|call| call.name.as_str()),
                    "identical tool batch repeated; injecting steering note"
                );
                session.push_message(Message::user(format!(
                    "Note: the previous tool call(s) were repeated {} times in a row with \
                     identical arguments. If the results are also unchanged, the repetition \
                     is not making progress: change the approach — call the tool with \
                     different arguments, use a different tool, or end the turn with a reply \
                     to the user.",
                    identical_batch_streak
                )));
            }
        }
        if let Some(journal) = &options.journal {
            journal
                .commit(context_recovery::prompt_checkpoint(
                    session,
                    session.prompt_items(),
                    &turn_user_inputs,
                ))
                .await
                .map_err(AgentError::Provider)?;
        }

        // If the turn was cancelled while tools were running, keep the
        // interrupted tool results above and stop without another model call.
        if options
            .cancel_token
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            session.end_turn();
            return Err(AgentError::Aborted);
        }
        if options
            .end_turn_requested
            .as_ref()
            .is_some_and(|requested| requested.load(std::sync::atomic::Ordering::Acquire))
        {
            session.end_turn();
            return Ok(());
        }
    }
}

/// Sends a minimal provider probe request used by onboarding and configuration checks.
pub async fn test_model_connection(
    provider: &dyn ModelProviderSDK,
    model: &Model,
    model_profile: devo_protocol::ModelProfileKey,
    request_model: &str,
    prompt: &str,
) -> Result<String, AgentError> {
    let ResolvedReasoningRequest {
        request_model: _,
        request_thinking,
        request_reasoning_effort,
        extra_body,
        effective_reasoning_effort: _,
    } = model.resolve_reasoning_effort_selection(None);
    let request = ModelRequest {
        model_slug: model_profile,
        model: request_model.to_string(),
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
        request_thinking,
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

#[cfg(test)]
mod tests;

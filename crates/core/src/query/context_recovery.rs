//! Apply and persist smaller model-facing histories after context overflow.

use std::sync::Arc;

use devo_protocol::{ModelRequest, ProviderWireApi};
use devo_provider::ModelProviderSDK;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::SessionState;
use crate::durable_execution::{ExecutionCounters, ExecutionRecord, ToolIntentJournal};
use crate::history::TokenInfo;
use crate::history::compaction::{
    CompactAction, CompactionConfig, CompactionKind, compact_history_with_inputs,
    estimated_history_tokens,
};
use crate::history::response_items_to_messages;
use crate::history::summarizer::DefaultHistorySummarizer;

use super::event::{EventCallback, QueryEvent, emit_query_event};

pub(super) const OVERFLOW_COMPACTION_KINDS: [CompactionKind; 2] =
    [CompactionKind::Proactive, CompactionKind::OverflowRecovery];

/// Output allowance carried by the selected wire API, including request-body overrides.
pub(super) fn requested_output_tokens(request: &ModelRequest, wire: ProviderWireApi) -> usize {
    let extra = request.extra_body.as_ref();
    let (limit, default) = match wire {
        ProviderWireApi::OpenAIChatCompletions => (
            extra.and_then(|v| {
                v.get("max_completion_tokens")
                    .or_else(|| v.get("max_tokens"))
            }),
            request.max_tokens,
        ),
        ProviderWireApi::OpenAIResponses => (extra.and_then(|v| v.get("max_output_tokens")), 0),
        ProviderWireApi::AnthropicMessages => {
            (extra.and_then(|v| v.get("max_tokens")), request.max_tokens)
        }
        ProviderWireApi::GoogleGenerativeAi => (
            extra.and_then(|v| v.pointer("/generationConfig/maxOutputTokens")),
            request.max_tokens,
        ),
    };
    limit
        .and_then(serde_json::Value::as_u64)
        .and_then(|v| usize::try_from(v).ok())
        .unwrap_or(default)
}

pub(super) struct CompactionModelRequest<'a> {
    pub(super) turn_user_inputs: &'a [crate::ResponseItem],
    pub(super) journal: Option<&'a dyn ToolIntentJournal>,
    pub(super) provider: &'a Arc<dyn ModelProviderSDK>,
    pub(super) model_slug: &'a str,
    pub(super) request_model: &'a str,
    pub(super) max_tokens: usize,
}

pub(super) fn prompt_checkpoint(
    session: &SessionState,
    items: Vec<crate::ResponseItem>,
    inputs: &[crate::ResponseItem],
) -> ExecutionRecord {
    let turn_input_indices = crate::history::compaction::turn_input_indices(&items, inputs)
        .into_iter()
        .collect();
    ExecutionRecord::PromptCheckpoint {
        items,
        turn_input_indices,
        counters: Some(ExecutionCounters::capture(session)),
    }
}

/// Apply the first usable compaction and report whether prompt history changed.
/// If a mode cannot reduce it, try the next mode before retrying the query.
pub(super) async fn summarize_and_compact(
    session: &mut SessionState,
    on_event: &Option<EventCallback>,
    model: CompactionModelRequest<'_>,
    kinds: impl IntoIterator<Item = CompactionKind>,
    cancel_token: Option<&CancellationToken>,
) -> bool {
    let mut kinds = kinds.into_iter().peekable();
    if kinds.peek().is_none() {
        return false;
    }
    let items = session.prompt_items();
    let token_info = TokenInfo {
        input_tokens: session.total_input_tokens,
        cached_input_tokens: session.total_cache_read_tokens,
        output_tokens: session.total_output_tokens,
    };
    let mut failure = "Context compaction skipped: nothing to compact".to_string();
    emit_query_event(on_event, QueryEvent::ContextCompactionStarted).await;

    for kind in kinds {
        let config = CompactionConfig {
            budget: session.config.token_budget.clone(),
            kind,
        };
        let max_tokens = config.summary_output_limit(model.max_tokens);
        let summarizer = DefaultHistorySummarizer::with_models(
            Arc::clone(model.provider),
            model.model_slug,
            model.request_model,
            max_tokens,
        );
        let compacted_items = match compact_history_with_inputs(
            &items,
            &token_info,
            &summarizer,
            &config,
            model.turn_user_inputs,
            cancel_token,
        )
        .await
        {
            Ok(CompactAction::Replaced(compacted_items)) => compacted_items,
            Ok(CompactAction::Skipped) => continue,
            Err(error) => {
                failure = error.to_string();
                break;
            }
        };
        let before_tokens = estimated_history_tokens(&items);
        let after_tokens = estimated_history_tokens(&compacted_items);
        if after_tokens >= before_tokens {
            failure = "Context compaction did not reduce the estimated prompt size".to_string();
            continue;
        }

        if let Some(journal) = model.journal
            && let Err(error) = journal
                .commit(prompt_checkpoint(
                    session,
                    compacted_items.clone(),
                    model.turn_user_inputs,
                ))
                .await
        {
            failure = format!("failed to persist compaction checkpoint: {error}");
            break;
        }

        session.set_prompt_messages(response_items_to_messages(&compacted_items));
        info!(
            ?kind,
            before_tokens, after_tokens, "compacted model prompt history"
        );
        emit_query_event(
            on_event,
            QueryEvent::ContextCompactionCompleted { compacted_items },
        )
        .await;
        return true;
    }

    warn!(message = %failure, "context compaction did not complete");
    emit_query_event(
        on_event,
        QueryEvent::ContextCompactionFailed { message: failure },
    )
    .await;
    false
}

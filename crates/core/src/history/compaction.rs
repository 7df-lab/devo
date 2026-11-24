//! Replace completed history with a checkpoint at a model-call boundary.
//! Rolling compaction keeps the latest user input and a budgeted recent tail.
//! If the provider still rejects the request, release the completed tail too.

mod chunks;
#[cfg(test)]
mod long_turn_tests;
mod overflow;
pub(crate) mod prefix;
mod references;
use overflow::is_user_input;
pub(crate) use overflow::is_user_message;

use devo_protocol::approx_tokens_from_byte_count;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use crate::context::ContextualUserFragment;
use crate::context::TokenBudget;
use crate::context::compaction_summary::CompactionSummary;
use crate::response_item::ResponseItem;

use devo_protocol::RequestContent;
use devo_protocol::RequestMessage;
use devo_protocol::RequestRole;
use devo_protocol::normalize_tool_result_messages;

use super::TokenInfo;

const SUMMARIZATION_PROMPT: &str = include_str!("../../prompts/compact/prompt.md");
/// Upper bound for the verbatim recent history in a rolling checkpoint.
const MAX_RECENT_TOKENS: usize = 20_000;

// ---------------------------------------------------------------------------
// CompactionError
// ---------------------------------------------------------------------------

/// Errors that can occur during history compaction.
#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
pub enum CompactionError {
    /// The summarizer provider call failed.
    #[error("summarization failed: {message}")]
    SummarizationFailed {
        /// Human-readable failure description.
        message: String,
    },
    /// The summarizer's context window was exceeded by the input.
    #[error("summarizer context window exceeded")]
    ContextTooLong,
    /// The summarizer returned an empty response.
    #[error("summarizer returned empty response")]
    EmptyResponse,
    /// Compaction was canceled by the caller.
    #[error("compaction canceled")]
    Canceled,
    /// Compaction is not possible after exhausting retries.
    #[error("compaction not possible after {retries} retries")]
    NotPossible {
        /// Number of retries attempted.
        retries: u32,
    },
}

// ---------------------------------------------------------------------------
// HistorySummarizer trait
// ---------------------------------------------------------------------------

/// Pluggable interface for the LLM call that produces a compaction summary.
///
/// Implementations are provided by the caller (e.g. the query loop) so that
/// this module does not depend directly on a specific provider SDK.
#[allow(clippy::double_must_use)]
#[async_trait]
pub trait HistorySummarizer: Send + Sync {
    /// Send `messages` (to-compact history followed by a developer compaction
    /// prompt) to the model and return the generated summary text.
    ///
    /// When `cancel_token` is set, implementations should return
    /// [`CompactionError::Canceled`] promptly if it fires while the provider
    /// call is in flight (typically by racing the request against
    /// `cancel_token.cancelled()`).
    async fn summarize(
        &self,
        messages: Vec<RequestMessage>,
        cancel_token: Option<&CancellationToken>,
    ) -> Result<String, CompactionError>;
}

// ---------------------------------------------------------------------------
// CompactionConfig
// ---------------------------------------------------------------------------

/// Configuration for the compaction process.
#[derive(Debug, Clone)]
pub struct CompactionConfig {
    /// Token budget used to decide whether compaction is needed.
    pub budget: TokenBudget,
    /// How compaction was triggered — automatic or proactive.
    pub kind: CompactionKind,
}

impl CompactionConfig {
    /// Reserve a bounded generation for the checkpoint itself.
    pub fn summary_output_limit(&self, model_limit: usize) -> usize {
        model_limit
            .min(4096)
            .min((self.budget.context_window / 8).max(1))
    }
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            budget: TokenBudget::default(),
            kind: CompactionKind::Auto,
        }
    }
}

// ---------------------------------------------------------------------------
// CompactionKind — how compaction was triggered
// ---------------------------------------------------------------------------

/// Select the pressure check and the amount of recent work to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionKind {
    /// Use the reported usage threshold and keep a budgeted recent tail.
    Auto,
    /// Make a rolling checkpoint when requested or when preflight detects pressure.
    Proactive,
    /// Keep the current input and incomplete tool batches only.
    OverflowRecovery,
}

// ---------------------------------------------------------------------------
// CompactAction — describes how to act on a compaction decision
// ---------------------------------------------------------------------------

/// Describes the outcome of a single compaction attempt.
#[derive(Debug)]
pub enum CompactAction {
    /// Compaction succeeded, yielding items to replace the history.
    Replaced(Vec<ResponseItem>),
    /// Compaction was not needed (history is within budget).
    Skipped,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Determine whether compaction should run for the given token info.
pub fn should_compact(token_info: &TokenInfo, budget: &TokenBudget) -> bool {
    if token_info.input_tokens == 0 && token_info.cached_input_tokens == 0 {
        return false;
    }
    let current = token_info
        .input_tokens
        .saturating_add(token_info.cached_input_tokens)
        .saturating_add(token_info.output_tokens);
    budget.should_compact(current)
}

/// Compact the history using an LLM-backed summarizer.
pub async fn compact_history(
    items: &[ResponseItem],
    token_info: &TokenInfo,
    summarizer: &dyn HistorySummarizer,
    config: &CompactionConfig,
    cancel_token: Option<&CancellationToken>,
) -> Result<CompactAction, CompactionError> {
    compact_history_with_inputs(
        items,
        token_info,
        summarizer,
        config,
        &[],
        /*prefix*/ None,
        cancel_token,
    )
    .await
}

/// Locate the exact resolved parts, in chronological order, in this checkpoint.
pub(crate) fn turn_input_indices(
    items: &[ResponseItem],
    inputs: &[ResponseItem],
) -> std::collections::BTreeSet<usize> {
    let mut pinned = std::collections::BTreeSet::new();
    let mut end = items.len();
    for input in inputs.iter().rev() {
        if let Some(i) = items[..end].iter().rposition(|item| item == input) {
            pinned.insert(i);
            end = i;
        }
    }
    pinned
}

/// Protect the running turn's original user input and subsequently delivered human steers.
pub(crate) async fn compact_history_with_inputs(
    items: &[ResponseItem],
    token_info: &TokenInfo,
    summarizer: &dyn HistorySummarizer,
    config: &CompactionConfig,
    turn_user_inputs: &[ResponseItem],
    prefix: Option<prefix::PrefixAttempt<'_>>,
    cancel_token: Option<&CancellationToken>,
) -> Result<CompactAction, CompactionError> {
    // For auto compaction, skip if already within budget.
    // Proactive compaction always proceeds regardless of budget.
    if config.kind == CompactionKind::Auto && !should_compact(token_info, &config.budget) {
        return Ok(CompactAction::Skipped);
    }

    let mut pinned = turn_input_indices(items, turn_user_inputs);
    if pinned.is_empty() {
        pinned
            .extend(overflow::latest_user_input_range(items).filter(|&i| is_user_input(&items[i])));
    }
    let safe_boundaries = super::transactions::completed_boundaries(items);
    let complete_end = *safe_boundaries.last().expect("zero is a safe boundary");
    let boundary = match config.kind {
        CompactionKind::OverflowRecovery => complete_end,
        CompactionKind::Auto | CompactionKind::Proactive => {
            let tail_budget = (config.budget.input_budget() / 4).min(MAX_RECENT_TOKENS);
            let mut desired = recent_tail_start(items, tail_budget);
            if config.kind == CompactionKind::Proactive {
                // A forced checkpoint can always incorporate earlier turns.
                // A long current turn may advance the cut beyond this minimum.
                desired = desired.max(pinned.first().copied().unwrap_or(0));
            }
            // A completed batch larger than the tail allowance is summarized in
            // full. An open batch must stay verbatim, however large it is.
            if desired == 0 && config.kind == CompactionKind::Proactive {
                // Under real provider pressure even a locally small tail may be too large.
                // Keep the newest complete batch when an older prefix can be summarized.
                let last_batch = safe_boundaries
                    .iter()
                    .rev()
                    .copied()
                    .find(|&i| i < items.len())
                    .unwrap_or(0);
                if (0..last_batch).any(|i| !pinned.contains(&i)) {
                    last_batch
                } else {
                    complete_end
                }
            } else {
                safe_boundaries
                    .iter()
                    .copied()
                    .find(|&i| i >= desired)
                    .unwrap_or(complete_end)
            }
        }
    };
    let Some(first_replaced) = (0..boundary).find(|i| !pinned.contains(i)) else {
        return Ok(CompactAction::Skipped);
    };
    // Include the original instruction when it lies in the selected prefix so
    // the summarizer can tell which results matter. It also survives verbatim.
    let messages = summarizer_request_messages(&items[..boundary]);
    let cached = match prefix {
        Some(prefix) => {
            prefix
                .summarize(
                    items,
                    &safe_boundaries,
                    boundary,
                    first_replaced,
                    &messages,
                    cancel_token,
                )
                .await?
        }
        None => None,
    };
    let (boundary, summary) = match cached {
        Some(result) => result,
        None => (
            boundary,
            chunks::summarize(summarizer, messages, cancel_token).await?,
        ),
    };
    if cancel_token.is_some_and(CancellationToken::is_cancelled) {
        return Err(CompactionError::Canceled);
    }
    let summary = references::attach(summary, &items[..boundary]);
    let mut compacted = vec![CompactionSummary::new(summary).to_response_item()];
    compacted.extend(
        items
            .iter()
            .enumerate()
            .filter(|(i, _)| *i >= boundary || pinned.contains(i))
            .map(|(_, item)| item.clone()),
    );
    Ok(CompactAction::Replaced(compacted))
}

async fn sleep_cancellable(
    delay: Duration,
    cancel_token: Option<&CancellationToken>,
) -> Result<(), CompactionError> {
    let Some(cancel_token) = cancel_token else {
        sleep(delay).await;
        return Ok(());
    };
    tokio::select! {
        biased;
        () = cancel_token.cancelled() => Err(CompactionError::Canceled),
        () = sleep(delay) => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Returns the start of the newest suffix that fits the raw-history allowance.
fn recent_tail_start(items: &[ResponseItem], mut remaining: usize) -> usize {
    for (index, item) in items.iter().enumerate().rev() {
        let size = estimate_item_tokens(item);
        if size > remaining {
            return index + 1;
        }
        remaining -= size;
    }
    0
}

fn summarizer_request_messages(to_compact: &[ResponseItem]) -> Vec<RequestMessage> {
    let without_digests = super::harness_digest_strip::exclude_harness_digests(to_compact);
    let mut messages: Vec<RequestMessage> =
        without_digests.iter().map(RequestMessage::from).collect();
    merge_consecutive_assistant_messages(&mut messages);
    normalize_tool_result_messages(&mut messages);
    messages.push(RequestMessage {
        role: RequestRole::Developer.as_str().to_string(),
        content: vec![RequestContent::Text {
            text: SUMMARIZATION_PROMPT.trim().to_string(),
        }],
    });
    messages
}

/// Merges consecutive assistant `RequestMessage`s into a single message by
/// concatenating their content arrays.
///
/// This mirrors the same logic in `history/mod.rs` and is needed because
/// `ResponseItem` → `RequestMessage` conversion can split a single assistant
/// turn (text + tool calls) into multiple consecutive assistant messages,
/// which provider APIs reject.
fn merge_consecutive_assistant_messages(messages: &mut Vec<RequestMessage>) {
    let assistant_role = "assistant";
    let capacity = messages.len();
    let previous = std::mem::replace(messages, Vec::with_capacity(capacity));
    for mut message in previous {
        match messages.last_mut() {
            Some(last) if last.role == assistant_role && message.role == assistant_role => {
                last.content.append(&mut message.content);
            }
            _ => messages.push(message),
        }
    }
}

struct JsonByteCounter {
    bytes: usize,
}

impl std::io::Write for JsonByteCounter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.bytes += buf.len();
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Estimates model-facing history size for comparing compaction results.
pub(crate) fn estimated_history_tokens(items: &[ResponseItem]) -> usize {
    items.iter().map(estimate_item_tokens).sum()
}

/// Estimates the byte-length-based token count for a single item.
fn estimate_item_tokens(item: &ResponseItem) -> usize {
    let bytes = match item {
        ResponseItem::Reason { text } => text.len(),
        ResponseItem::Message(msg) => {
            let mut bytes = 0;
            let mut text_blocks = 0;
            for block in &msg.content {
                let text = match block {
                    devo_protocol::ContentBlock::Text { text }
                    | devo_protocol::ContentBlock::Reasoning { text } => text,
                    devo_protocol::ContentBlock::ProviderReasoning { .. }
                    | devo_protocol::ContentBlock::ToolUse { .. }
                    | devo_protocol::ContentBlock::HostedToolUse { .. }
                    | devo_protocol::ContentBlock::ToolResult { .. }
                    | devo_protocol::ContentBlock::Image { .. } => {
                        let mut counter = JsonByteCounter { bytes: 0 };
                        serde_json::to_writer(&mut counter, block).expect(
                            "serializing a content block to a byte counter should not fail",
                        );
                        bytes += counter.bytes;
                        continue;
                    }
                };
                if text_blocks > 0 {
                    bytes += 1;
                }
                bytes += text.len();
                text_blocks += 1;
            }
            bytes
        }
        ResponseItem::ToolCall { name, input, .. } => {
            let mut counter = JsonByteCounter { bytes: 0 };
            serde_json::to_writer(&mut counter, input)
                .expect("serializing serde_json::Value to byte counter should not fail");
            name.len() + 2 + counter.bytes
        }
        ResponseItem::ToolCallOutput { content, .. } => content.len(),
    };
    approx_tokens_from_byte_count(bytes)
        .try_into()
        .unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use std::hint::black_box;
    use std::time::Instant;

    use devo_protocol::Message;
    use devo_protocol::RequestContent;
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::context::TokenBudget;
    use crate::response_item::ResponseItem;

    #[test]
    fn compact_builds_merged_messages() {
        let items = [
            ResponseItem::Message(Message::user("hello")),
            // Split assistant turn — text + two tool calls
            ResponseItem::Message(Message::assistant_text("ok")),
            ResponseItem::ToolCall {
                id: "call-1".into(),
                name: "read".into(),
                input: serde_json::json!({"filePath": "/tmp/a.txt"}),
            },
            ResponseItem::ToolCall {
                id: "call-2".into(),
                name: "bash".into(),
                input: serde_json::json!({"cmd": "date"}),
            },
            ResponseItem::ToolCallOutput {
                tool_use_id: "call-1".into(),
                content: "content".into(),
                is_error: false,
            },
            ResponseItem::ToolCallOutput {
                tool_use_id: "call-2".into(),
                content: "output".into(),
                is_error: false,
            },
        ];

        let mut messages: Vec<RequestMessage> = items.iter().map(RequestMessage::from).collect();
        merge_consecutive_assistant_messages(&mut messages);

        // user, assistant(merged), user, user
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content.len(), 1);

        assert_eq!(messages[1].role, "assistant");
        assert_eq!(messages[1].content.len(), 3);
        assert!(matches!(
            &messages[1].content[0],
            RequestContent::Text { .. }
        ));
        assert!(
            matches!(&messages[1].content[1], RequestContent::ToolUse { id, .. } if id == "call-1")
        );
        assert!(
            matches!(&messages[1].content[2], RequestContent::ToolUse { id, .. } if id == "call-2")
        );

        assert_eq!(messages[2].role, "user");
        assert_eq!(messages[2].content.len(), 1);
        assert!(
            matches!(&messages[2].content[0], RequestContent::ToolResult { tool_use_id, .. } if tool_use_id == "call-1")
        );
        assert_eq!(messages[3].role, "user");
        assert_eq!(messages[3].content.len(), 1);
        assert!(
            matches!(&messages[3].content[0], RequestContent::ToolResult { tool_use_id, .. } if tool_use_id == "call-2")
        );
    }

    #[test]
    fn compaction_summarizer_messages_put_prompt_last() {
        let items = vec![
            ResponseItem::Message(Message::user("hello")),
            ResponseItem::Message(Message::assistant_text("ok")),
            ResponseItem::ToolCall {
                id: "call-1".into(),
                name: "read".into(),
                input: serde_json::json!({"filePath": "/tmp/a.txt"}),
            },
            ResponseItem::ToolCallOutput {
                tool_use_id: "call-1".into(),
                content: "content".into(),
                is_error: false,
            },
        ];

        let messages = summarizer_request_messages(&items);
        let last = messages
            .last()
            .expect("summarizer messages should not be empty");

        assert_eq!(last.role, RequestRole::Developer.as_str());
        assert!(
            matches!(&last.content[0], RequestContent::Text { text } if text.contains("CONTEXT CHECKPOINT COMPACTION"))
        );
        assert_ne!(messages[0].role, RequestRole::Developer.as_str());
    }

    #[test]
    fn should_compact_false_when_no_tokens() {
        let info = TokenInfo::default();
        let budget = TokenBudget::new(200_000, 8192);
        assert!(!should_compact(&info, &budget));
    }

    #[test]
    fn split_by_user_message_budget_all_preserved() {
        let items = vec![
            ResponseItem::Message(Message::user("short")),
            ResponseItem::Message(Message::assistant_text("ok")),
        ];
        let (compact, preserve) = items.split_at(recent_tail_start(&items, 10_000));
        assert!(compact.is_empty());
        assert_eq!(preserve.len(), 2);
    }

    #[test]
    fn split_by_user_message_budget_boundary() {
        let items = vec![
            ResponseItem::Message(Message::user("a".repeat(400))), // ~100 tokens
            ResponseItem::Message(Message::assistant_text("b".repeat(400))),
            ResponseItem::Message(Message::user("c".repeat(400))),
            ResponseItem::Message(Message::assistant_text("d".repeat(400))),
        ];

        // Budget enough for about 2 items.
        let (compact, preserve) = items.split_at(recent_tail_start(&items, 200));
        assert!(!preserve.is_empty());

        // The preserved part should be the tail.
        assert_eq!(preserve.len() + compact.len(), items.len());
    }

    #[tokio::test]
    async fn auto_compaction_preserves_tail_by_token_budget_not_latest_user_only() {
        struct StubSummarizer;

        #[async_trait]
        impl HistorySummarizer for StubSummarizer {
            async fn summarize(
                &self,
                _messages: Vec<RequestMessage>,
                _cancel_token: Option<&CancellationToken>,
            ) -> Result<String, CompactionError> {
                Ok("summary".to_string())
            }
        }

        let large_tail = "x".repeat(40_000);
        let items = vec![
            ResponseItem::Message(Message::user("old user")),
            ResponseItem::Message(Message::assistant_text("old assistant")),
            ResponseItem::Message(Message::user("latest user")),
            ResponseItem::Message(Message::assistant_text(large_tail.clone())),
            ResponseItem::Message(Message::assistant_text(large_tail)),
            ResponseItem::Message(Message::assistant_text("last observation")),
        ];
        let token_info = TokenInfo {
            input_tokens: 200,
            cached_input_tokens: 0,
            output_tokens: 0,
        };
        let config = CompactionConfig {
            budget: TokenBudget {
                auto_compact_token_limit: Some(100),
                ..TokenBudget::new(200_000, 8192)
            },
            kind: CompactionKind::Auto,
        };

        let action = compact_history(
            &items,
            &token_info,
            &StubSummarizer,
            &config,
            /*cancel_token*/ None,
        )
        .await
        .expect("auto compaction should succeed");

        let proactive_config = CompactionConfig {
            budget: config.budget.clone(),
            kind: CompactionKind::Proactive,
        };
        let proactive_action = compact_history(
            &items,
            &token_info,
            &StubSummarizer,
            &proactive_config,
            /*cancel_token*/ None,
        )
        .await
        .expect("proactive compaction should succeed");

        let (CompactAction::Replaced(auto), CompactAction::Replaced(forced)) =
            (action, proactive_action)
        else {
            panic!("expected both modes to replace history");
        };
        assert_eq!(auto, forced);
        assert_eq!(
            auto[1..],
            [items[2].clone(), items[4].clone(), items[5].clone()]
        );
    }

    /// Verifies: cancel token aborts an in-flight history summarizer.
    #[tokio::test]
    async fn compact_history_returns_canceled_when_token_fires() {
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;
        use std::sync::atomic::Ordering;

        struct HangingSummarizer {
            entered: Arc<AtomicBool>,
        }

        #[async_trait]
        impl HistorySummarizer for HangingSummarizer {
            async fn summarize(
                &self,
                _messages: Vec<RequestMessage>,
                cancel_token: Option<&CancellationToken>,
            ) -> Result<String, CompactionError> {
                self.entered.store(true, Ordering::SeqCst);
                let Some(cancel_token) = cancel_token else {
                    std::future::pending::<()>().await;
                    unreachable!("summarizer should be canceled")
                };
                cancel_token.cancelled().await;
                Err(CompactionError::Canceled)
            }
        }

        let items = vec![
            ResponseItem::Message(Message::user("first")),
            ResponseItem::Message(Message::assistant_text("reply")),
            ResponseItem::Message(Message::user("latest")),
        ];
        let token_info = TokenInfo {
            input_tokens: 10,
            cached_input_tokens: 0,
            output_tokens: 5,
        };
        let config = CompactionConfig {
            budget: TokenBudget::new(200_000, 8192),
            kind: CompactionKind::Proactive,
        };
        let entered = Arc::new(AtomicBool::new(false));
        let summarizer = HangingSummarizer {
            entered: Arc::clone(&entered),
        };
        let cancel_token = CancellationToken::new();
        let cancel_for_task = cancel_token.clone();
        let compact = tokio::spawn(async move {
            compact_history(
                &items,
                &token_info,
                &summarizer,
                &config,
                Some(&cancel_for_task),
            )
            .await
        });

        for _ in 0..50 {
            if entered.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            entered.load(Ordering::SeqCst),
            "summarizer should start before cancel"
        );
        cancel_token.cancel();
        let result = tokio::time::timeout(Duration::from_secs(2), compact)
            .await
            .expect("compaction should finish after cancel")
            .expect("compaction task should not panic");
        assert!(matches!(result, Err(CompactionError::Canceled)));
    }

    #[tokio::test]
    async fn proactive_compaction_summarizes_all_history_and_preserves_latest_user_suffix() {
        struct StubSummarizer;

        #[async_trait]
        impl HistorySummarizer for StubSummarizer {
            async fn summarize(
                &self,
                messages: Vec<RequestMessage>,
                _cancel_token: Option<&CancellationToken>,
            ) -> Result<String, CompactionError> {
                assert_eq!(messages.len(), 3);
                let last = messages
                    .last()
                    .expect("summarizer messages should not be empty");
                assert_eq!(last.role, RequestRole::Developer.as_str());
                assert!(
                    matches!(&last.content[0], RequestContent::Text { text } if text.contains("CONTEXT CHECKPOINT COMPACTION"))
                );
                Ok("summary".to_string())
            }
        }

        let items = vec![
            ResponseItem::Message(Message::user("first")),
            ResponseItem::Message(Message::assistant_text("reply")),
            ResponseItem::Message(Message::user("latest user")),
        ];
        let token_info = TokenInfo {
            input_tokens: 10,
            cached_input_tokens: 0,
            output_tokens: 5,
        };
        let config = CompactionConfig {
            budget: TokenBudget::new(200_000, 8192),
            kind: CompactionKind::Proactive,
        };

        let action = compact_history(
            &items,
            &token_info,
            &StubSummarizer,
            &config,
            /*cancel_token*/ None,
        )
        .await
        .expect("proactive compaction should succeed");

        match action {
            CompactAction::Replaced(compacted) => {
                assert_eq!(compacted.len(), 2);
                assert!(matches!(compacted[0], ResponseItem::Message(_)));
                assert_eq!(
                    compacted[1],
                    ResponseItem::Message(Message::user("latest user"))
                );
            }
            CompactAction::Skipped => panic!("expected proactive compaction to replace history"),
        }
    }

    #[tokio::test]
    async fn checkpoint_keeps_a_partially_returned_parallel_batch_out_of_the_summary() {
        struct CaptureSummary(std::sync::Mutex<Vec<RequestMessage>>);
        #[async_trait]
        impl HistorySummarizer for CaptureSummary {
            async fn summarize(
                &self,
                messages: Vec<RequestMessage>,
                _cancel_token: Option<&CancellationToken>,
            ) -> Result<String, CompactionError> {
                *self.0.lock().unwrap() = messages;
                Ok("completed work".into())
            }
        }
        let input = ResponseItem::Message(Message::user("Track this job"));
        let in_flight = [
            ResponseItem::Reason {
                text: "pending-thought".into(),
            },
            ResponseItem::Message(Message {
                role: devo_protocol::Role::Assistant,
                content: vec![devo_protocol::ContentBlock::ProviderReasoning {
                    provider: "test".into(),
                    payload: serde_json::json!({"signature":"pending-signed"}),
                }],
            }),
            ResponseItem::Message(Message::assistant_text("pending-step")),
            ResponseItem::ToolCall {
                id: "pending-a".into(),
                name: "read".into(),
                input: serde_json::json!({}),
            },
            ResponseItem::ToolCall {
                id: "pending-b".into(),
                name: "read".into(),
                input: serde_json::json!({}),
            },
            ResponseItem::ToolCallOutput {
                tool_use_id: "pending-b".into(),
                content: "partial result".into(),
                is_error: false,
            },
        ];
        let mut items = vec![
            input.clone(),
            ResponseItem::Message(Message::assistant_text("Earlier observation")),
        ];
        items.extend_from_slice(&in_flight);
        for kind in [CompactionKind::Proactive, CompactionKind::OverflowRecovery] {
            let summarizer = CaptureSummary(Default::default());
            let action = compact_history(
                &items,
                &TokenInfo::default(),
                &summarizer,
                &CompactionConfig {
                    kind,
                    ..Default::default()
                },
                /*cancel_token*/ None,
            )
            .await
            .unwrap();
            let CompactAction::Replaced(retained) = action else {
                panic!("expected checkpoint");
            };
            let mut expected = vec![
                CompactionSummary::new("completed work").to_response_item(),
                input.clone(),
            ];
            expected.extend_from_slice(&in_flight);
            assert_eq!(retained, expected);
            let captured = serde_json::to_string(&*summarizer.0.lock().unwrap()).unwrap();
            assert!(
                captured.contains("Earlier observation") && captured.contains("Track this job")
            );
            assert!(!captured.contains("pending-"));
        }
    }

    #[test]
    fn compact_action_debug() {
        let action = CompactAction::Skipped;
        assert_eq!(format!("{:?}", action), "Skipped");
    }

    #[test]
    fn estimate_item_tokens_for_different_variants() {
        let reason = ResponseItem::Reason {
            text: "thinking deeply".into(),
        };
        assert!(estimate_item_tokens(&reason) > 0);

        let msg = ResponseItem::Message(Message::user("hello world"));
        assert!(estimate_item_tokens(&msg) > 0);

        let tc = ResponseItem::ToolCall {
            id: "tc-1".into(),
            name: "bash".into(),
            input: serde_json::json!({"cmd": "ls"}),
        };
        assert!(estimate_item_tokens(&tc) > 0);

        let tco = ResponseItem::ToolCallOutput {
            tool_use_id: "tc-1".into(),
            content: "done".into(),
            is_error: false,
        };
        assert!(estimate_item_tokens(&tco) > 0);
    }

    #[test]
    fn estimate_item_tokens_for_tool_call_matches_serialized_input_len() {
        let input = serde_json::json!({
            "cmd": "printf 'hello'",
            "cwd": "/tmp",
            "timeout_ms": 1000,
            "labels": ["a", "b"]
        });
        let item = ResponseItem::ToolCall {
            id: "tc-1".into(),
            name: "shell_command".into(),
            input: input.clone(),
        };

        let expected_bytes = "shell_command".len() + 2 + input.to_string().len();
        assert_eq!(
            estimate_item_tokens(&item),
            approx_tokens_from_byte_count(expected_bytes) as usize
        );
    }

    #[test]
    #[ignore]
    fn bench_estimate_item_tokens_for_tool_calls() {
        let items = (0..500)
            .map(|index| ResponseItem::ToolCall {
                id: format!("tc-{index}"),
                name: "shell_command".into(),
                input: serde_json::json!({
                    "cmd": "rg --json \"ResponseItem::ToolCall\" crates/core/src -g !target",
                    "cwd": "/Users/tsiao/Desktop/devo-opt",
                    "timeout_ms": 10000,
                    "metadata": {
                        "index": index,
                        "capture_stdout": true,
                        "capture_stderr": true,
                        "labels": [
                            "history",
                            "compaction",
                            "token-estimate",
                            "tool-call"
                        ]
                    }
                }),
            })
            .collect::<Vec<_>>();

        let started = Instant::now();
        let mut total_tokens = 0;
        for _ in 0..2_000 {
            for item in &items {
                total_tokens += black_box(estimate_item_tokens(black_box(item)));
            }
        }
        let elapsed = started.elapsed();

        assert!(total_tokens > 0);
        println!(
            "estimate_item_tokens_for_tool_calls iterations=1000000 elapsed_ms={} per_call_us={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / 1_000_000.0
        );
    }
}

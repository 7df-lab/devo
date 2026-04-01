use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// A message representation used by the compaction layer.
///
/// The compactor works at the serialized message level so it stays
/// decoupled from the full message types in `core`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactMessage {
    pub role: String,
    pub content: String,
    /// Estimated token count for this message.
    pub token_estimate: usize,
}

/// Output of a compaction pass.
#[derive(Debug, Clone)]
pub struct CompactResult {
    pub messages: Vec<CompactMessage>,
    pub removed_count: usize,
    pub tokens_saved: usize,
}

/// A pluggable strategy for compressing conversation history.
///
/// Implementations can range from simple truncation to LLM-based
/// summarization, sliding windows, or tiered compression.
#[async_trait]
pub trait CompactStrategy: Send + Sync {
    async fn compact(
        &self,
        messages: Vec<CompactMessage>,
        budget: usize,
    ) -> anyhow::Result<CompactResult>;
}

/// Simplest strategy: drop oldest messages until under budget.
pub struct TruncateStrategy;

#[async_trait]
impl CompactStrategy for TruncateStrategy {
    async fn compact(
        &self,
        messages: Vec<CompactMessage>,
        budget: usize,
    ) -> anyhow::Result<CompactResult> {
        let total: usize = messages.iter().map(|m| m.token_estimate).sum();
        if total <= budget {
            return Ok(CompactResult {
                messages,
                removed_count: 0,
                tokens_saved: 0,
            });
        }

        let mut kept = Vec::new();
        let mut running = 0usize;
        let mut removed = 0usize;
        let mut saved = 0usize;

        // Keep the system/first message, then keep from the end
        for msg in messages.iter().rev() {
            if running + msg.token_estimate <= budget {
                running += msg.token_estimate;
                kept.push(msg.clone());
            } else {
                removed += 1;
                saved += msg.token_estimate;
            }
        }

        kept.reverse();

        Ok(CompactResult {
            messages: kept,
            removed_count: removed,
            tokens_saved: saved,
        })
    }
}

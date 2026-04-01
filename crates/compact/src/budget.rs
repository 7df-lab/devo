use serde::{Deserialize, Serialize};

/// Token budget configuration for a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenBudget {
    /// Maximum context window tokens the model supports.
    pub context_window: usize,
    /// Maximum tokens to reserve for the model's output.
    pub max_output_tokens: usize,
    /// Threshold at which auto-compaction is triggered
    /// (as a fraction of context_window, e.g. 0.8).
    pub compact_threshold: f64,
}

impl TokenBudget {
    pub fn new(context_window: usize, max_output_tokens: usize) -> Self {
        Self {
            context_window,
            max_output_tokens,
            compact_threshold: 0.8,
        }
    }

    /// Available tokens for input messages.
    pub fn input_budget(&self) -> usize {
        self.context_window.saturating_sub(self.max_output_tokens)
    }

    /// Whether compaction should fire given the current token usage.
    pub fn should_compact(&self, current_tokens: usize) -> bool {
        current_tokens as f64 > self.input_budget() as f64 * self.compact_threshold
    }
}

impl Default for TokenBudget {
    fn default() -> Self {
        Self::new(200_000, 16_000)
    }
}

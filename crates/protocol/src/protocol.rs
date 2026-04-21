use std::path::PathBuf;

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use strum_macros::Display;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientRequest<T> {
    pub id: serde_json::Value,
    pub method: String,
    pub params: T,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientNotification<T> {
    pub method: String,
    pub params: T,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuccessResponse<T> {
    pub id: serde_json::Value,
    pub result: T,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub id: serde_json::Value,
    pub error: ProtocolError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationEnvelope<T> {
    pub method: String,
    pub params: T,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerRequestEnvelope<T> {
    pub id: serde_json::Value,
    pub method: String,
    pub params: T,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum ProtocolErrorCode {
    #[error("NotInitialized")]
    NotInitialized,
    #[error("InvalidParams")]
    InvalidParams,
    #[error("SessionNotFound")]
    SessionNotFound,
    #[error("TurnNotFound")]
    TurnNotFound,
    #[error("TurnAlreadyRunning")]
    TurnAlreadyRunning,
    #[error("ApprovalNotFound")]
    ApprovalNotFound,
    #[error("PolicyDenied")]
    PolicyDenied,
    #[error("ContextLimitExceeded")]
    ContextLimitExceeded,
    #[error("NoActiveTurn")]
    NoActiveTurn,
    #[error("ExpectedTurnMismatch")]
    ExpectedTurnMismatch,
    #[error("ActiveTurnNotSteerable")]
    ActiveTurnNotSteerable,
    #[error("EmptyInput")]
    EmptyInput,
    #[error("InternalError")]
    InternalError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolError {
    pub code: ProtocolErrorCode,
    pub message: String,
    pub data: serde_json::Value,
}

#[derive(
    Debug, Clone, Copy, Display, Deserialize, Serialize, PartialEq, Eq, JsonSchema, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum ExecCommandSource {
    #[default]
    Agent,
    UserShell,
    UnifiedExecStartup,
    UnifiedExecInteraction,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FileChange {
    Add {
        content: String,
    },
    Delete {
        content: String,
    },
    Update {
        unified_diff: String,
        move_path: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize, Default, PartialEq, Eq, JsonSchema)]
pub struct TokenUsage {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub total_tokens: i64,
}

// Includes prompts, tools and space to call compact.
const BASELINE_TOKENS: i64 = 12000;

impl TokenUsage {
    pub fn is_zero(&self) -> bool {
        self.total_tokens == 0
    }

    pub fn cached_input(&self) -> i64 {
        self.cached_input_tokens.max(0)
    }

    pub fn non_cached_input(&self) -> i64 {
        (self.input_tokens - self.cached_input()).max(0)
    }

    /// Primary count for display as a single absolute value: non-cached input + output.
    pub fn blended_total(&self) -> i64 {
        (self.non_cached_input() + self.output_tokens.max(0)).max(0)
    }

    pub fn tokens_in_context_window(&self) -> i64 {
        self.total_tokens
    }

    /// Estimate the remaining user-controllable percentage of the model's context window.
    ///
    /// `context_window` is the total size of the model's context window.
    /// `BASELINE_TOKENS` should capture tokens that are always present in
    /// the context (e.g., system prompt and fixed tool instructions) so that
    /// the percentage reflects the portion the user can influence.
    ///
    /// This normalizes both the numerator and denominator by subtracting the
    /// baseline, so immediately after the first prompt the UI shows 100% left
    /// and trends toward 0% as the user fills the effective window.
    pub fn percent_of_context_window_remaining(&self, context_window: i64) -> i64 {
        if context_window <= BASELINE_TOKENS {
            return 0;
        }

        let effective_window = context_window - BASELINE_TOKENS;
        let used = (self.tokens_in_context_window() - BASELINE_TOKENS).max(0);
        let remaining = (effective_window - used).max(0);
        ((remaining as f64 / effective_window as f64) * 100.0)
            .clamp(0.0, 100.0)
            .round() as i64
    }

    /// In-place element-wise sum of token counts.
    pub fn add_assign(&mut self, other: &TokenUsage) {
        self.input_tokens += other.input_tokens;
        self.cached_input_tokens += other.cached_input_tokens;
        self.output_tokens += other.output_tokens;
        self.reasoning_output_tokens += other.reasoning_output_tokens;
        self.total_tokens += other.total_tokens;
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
pub struct TokenUsageInfo {
    pub total_token_usage: TokenUsage,
    pub last_token_usage: TokenUsage,
    pub model_context_window: Option<i64>,
}

impl TokenUsageInfo {
    pub fn new_or_append(
        info: &Option<TokenUsageInfo>,
        last: &Option<TokenUsage>,
        model_context_window: Option<i64>,
    ) -> Option<Self> {
        if info.is_none() && last.is_none() {
            return None;
        }

        let mut info = match info {
            Some(info) => info.clone(),
            None => Self {
                total_token_usage: TokenUsage::default(),
                last_token_usage: TokenUsage::default(),
                model_context_window,
            },
        };
        if let Some(last) = last {
            info.append_last_usage(last);
        }
        if let Some(model_context_window) = model_context_window {
            info.model_context_window = Some(model_context_window);
        }
        Some(info)
    }

    pub fn append_last_usage(&mut self, last: &TokenUsage) {
        self.total_token_usage.add_assign(last);
        self.last_token_usage = last.clone();
    }

    pub fn fill_to_context_window(&mut self, context_window: i64) {
        let previous_total = self.total_token_usage.total_tokens;
        let delta = (context_window - previous_total).max(0);

        self.model_context_window = Some(context_window);
        self.total_token_usage = TokenUsage {
            total_tokens: context_window,
            ..TokenUsage::default()
        };
        self.last_token_usage = TokenUsage {
            total_tokens: delta,
            ..TokenUsage::default()
        };
    }

    pub fn full_context_window(context_window: i64) -> Self {
        let mut info = Self {
            total_token_usage: TokenUsage::default(),
            last_token_usage: TokenUsage::default(),
            model_context_window: Some(context_window),
        };
        info.fill_to_context_window(context_window);
        info
    }
}

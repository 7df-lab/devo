//! Structured provider error classification.
//!
//! Implements L3-BEH-PROVIDER-001 §B6. Classifies provider failures into
//! recoverable and non-recoverable categories with retry hints.

use serde::{Deserialize, Serialize};

/// Structured error from a model provider invocation.
#[derive(Debug, Clone, thiserror::Error, Serialize, Deserialize)]
#[serde(tag = "error_kind", rename_all = "snake_case")]
pub enum ProviderError {
    #[error("authentication failed: {message}")]
    AuthenticationError {
        message: String,
        provider_name: Option<String>,
        status_code: Option<u16>,
    },

    #[error("rate limited: {message}")]
    RateLimitError {
        message: String,
        retry_after_seconds: Option<u64>,
        provider_name: Option<String>,
    },

    #[error("provider server error ({status_code:?}): {message}")]
    ProviderServerError {
        message: String,
        status_code: Option<u16>,
        provider_name: Option<String>,
    },

    #[error("provider timeout: {message}")]
    ProviderTimeoutError {
        message: String,
        provider_name: Option<String>,
    },

    #[error("context limit exceeded: {message}")]
    ContextLimitError {
        message: String,
        current_tokens: Option<u64>,
        limit: Option<u64>,
    },

    #[error("model not found: {model_name:?} — {message}")]
    ModelNotFoundError {
        message: String,
        model_name: Option<String>,
    },

    #[error("quota exceeded: {message}")]
    QuotaExceededError {
        message: String,
        provider_name: Option<String>,
    },

    #[error("content filtered: {message}")]
    ContentFilteredError {
        message: String,
        finish_reason: Option<String>,
    },

    #[error("invalid request: {message}")]
    InvalidRequestError {
        message: String,
        details: Option<String>,
    },

    #[error("stream error: {message}")]
    StreamError {
        message: String,
        bytes_received: Option<u64>,
    },

    #[error("unknown provider error: {message}")]
    UnknownError {
        message: String,
        status_code: Option<u16>,
    },
}

impl ProviderError {
    /// Whether retrying the request may succeed.
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            Self::RateLimitError { .. }
                | Self::ProviderServerError { .. }
                | Self::ProviderTimeoutError { .. }
                | Self::StreamError { .. }
        )
    }

    /// Whether this is a transient error that should be retried with backoff.
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::RateLimitError { .. }
                | Self::ProviderTimeoutError { .. }
                | Self::ProviderServerError {
                    status_code: Some(429),
                    ..
                }
                | Self::ProviderServerError {
                    status_code: Some(502),
                    ..
                }
                | Self::ProviderServerError {
                    status_code: Some(503),
                    ..
                }
                | Self::ProviderServerError {
                    status_code: Some(504),
                    ..
                }
        )
    }

    /// Suggested retry delay in seconds from the provider.
    pub fn retry_after_seconds(&self) -> Option<u64> {
        match self {
            Self::RateLimitError {
                retry_after_seconds,
                ..
            } => *retry_after_seconds,
            _ => None,
        }
    }

    /// Whether the error should be surfaced to the user.
    pub fn is_user_facing(&self) -> bool {
        !matches!(self, Self::StreamError { .. })
    }

    /// Machine-readable error code.
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::AuthenticationError { .. } => "AUTHENTICATION_ERROR",
            Self::RateLimitError { .. } => "RATE_LIMIT_ERROR",
            Self::ProviderServerError { .. } => "PROVIDER_SERVER_ERROR",
            Self::ProviderTimeoutError { .. } => "PROVIDER_TIMEOUT_ERROR",
            Self::ContextLimitError { .. } => "CONTEXT_LIMIT_ERROR",
            Self::ModelNotFoundError { .. } => "MODEL_NOT_FOUND_ERROR",
            Self::QuotaExceededError { .. } => "QUOTA_EXCEEDED_ERROR",
            Self::ContentFilteredError { .. } => "CONTENT_FILTERED_ERROR",
            Self::InvalidRequestError { .. } => "INVALID_REQUEST_ERROR",
            Self::StreamError { .. } => "STREAM_ERROR",
            Self::UnknownError { .. } => "UNKNOWN_ERROR",
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn rate_limit_is_recoverable() {
        let err = ProviderError::RateLimitError {
            message: "slow down".into(),
            retry_after_seconds: Some(30),
            provider_name: Some("anthropic".into()),
        };
        assert!(err.is_recoverable());
        assert!(err.is_transient());
        assert_eq!(err.retry_after_seconds(), Some(30));
    }

    #[test]
    fn auth_error_is_not_recoverable() {
        let err = ProviderError::AuthenticationError {
            message: "bad api key".into(),
            provider_name: Some("openai".into()),
            status_code: Some(401),
        };
        assert!(!err.is_recoverable());
        assert!(!err.is_transient());
        assert_eq!(err.error_code(), "AUTHENTICATION_ERROR");
    }

    #[test]
    fn server_error_503_is_transient() {
        let err = ProviderError::ProviderServerError {
            message: "service unavailable".into(),
            status_code: Some(503),
            provider_name: None,
        };
        assert!(err.is_transient());
    }

    #[test]
    fn context_limit_is_user_facing() {
        let err = ProviderError::ContextLimitError {
            message: "too many tokens".into(),
            current_tokens: Some(250000),
            limit: Some(200000),
        };
        assert!(err.is_user_facing());
        assert!(!err.is_recoverable());
    }

    #[test]
    fn stream_error_is_not_user_facing() {
        let err = ProviderError::StreamError {
            message: "connection reset".into(),
            bytes_received: Some(1024),
        };
        assert!(!err.is_user_facing());
        assert!(err.is_recoverable());
    }

    #[test]
    fn all_variants_have_distinct_codes() {
        let mut codes = std::collections::HashSet::new();
        let errors = vec![
            ProviderError::AuthenticationError {
                message: "".into(),
                provider_name: None,
                status_code: None,
            },
            ProviderError::RateLimitError {
                message: "".into(),
                retry_after_seconds: None,
                provider_name: None,
            },
            ProviderError::ProviderServerError {
                message: "".into(),
                status_code: None,
                provider_name: None,
            },
            ProviderError::ProviderTimeoutError {
                message: "".into(),
                provider_name: None,
            },
            ProviderError::ContextLimitError {
                message: "".into(),
                current_tokens: None,
                limit: None,
            },
            ProviderError::ModelNotFoundError {
                message: "".into(),
                model_name: None,
            },
            ProviderError::QuotaExceededError {
                message: "".into(),
                provider_name: None,
            },
            ProviderError::ContentFilteredError {
                message: "".into(),
                finish_reason: None,
            },
            ProviderError::InvalidRequestError {
                message: "".into(),
                details: None,
            },
            ProviderError::StreamError {
                message: "".into(),
                bytes_received: None,
            },
            ProviderError::UnknownError {
                message: "".into(),
                status_code: None,
            },
        ];
        for err in &errors {
            assert!(
                codes.insert(err.error_code()),
                "duplicate code: {}",
                err.error_code()
            );
        }
    }

    #[test]
    fn provider_error_serde_roundtrip() {
        let err = ProviderError::RateLimitError {
            message: "slow down".into(),
            retry_after_seconds: Some(30),
            provider_name: Some("anthropic".into()),
        };
        let json = serde_json::to_string(&err).expect("serialize");
        let restored: ProviderError = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.error_code(), "RATE_LIMIT_ERROR");
    }
}

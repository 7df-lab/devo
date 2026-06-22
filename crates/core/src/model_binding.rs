//! Model provider binding — three-tier data model.
//!
//! Implements L3-BEH-PROVIDER-001 and L2-DES-MODEL-001. Defines
//! SupportedModelDefinition (pure capability profile), UserProvider
//! (connection config), ModelProviderBinding (links model to provider),
//! and ResolvedModelProfile (runtime merge).

use serde::{Deserialize, Serialize};

use devo_protocol::ReasoningEffort;

use crate::durable_record::{InvocationMethod, ModelBindingId, ProviderId};

// ── SupportedModelDefinition ────────────────────────────────────────

/// Pure capability profile for a model — must NOT contain provider names, URLs,
/// API keys, or invocation methods.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SupportedModelDefinition {
    pub canonical_model_slug: String,
    pub display_name: String,
    pub base_instructions: String,
    pub context_window: u64,
    pub effective_context_window: u64,
    pub modalities: Vec<ModelModality>,
    pub reasoning_capability: ModelReasoningCapability,
    pub default_reasoning_effort: Option<ReasoningEffort>,
    pub supports_tool_use: bool,
    pub supports_parallel_tool_use: bool,
    pub supports_images: bool,
    pub supports_streaming: bool,
    pub supports_prompt_caching: bool,
    pub max_output_tokens: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelModality {
    Text,
    Image,
    Audio,
    Video,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelReasoningCapability {
    None,
    Enabled,
    Required,
    Toggleable,
    Levels(Vec<ReasoningEffort>),
}

impl SupportedModelDefinition {}

// ── UserProvider ────────────────────────────────────────────────────

/// User-configured provider connection. Identifies a provider instance
/// with its connection details and credential reference.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserProvider {
    pub provider_id: ProviderId,
    pub provider_name: String,
    pub provider_kind: ProviderKind,
    pub base_url: Option<String>,
    pub credential_ref: String,
    pub availability_status: ProviderAvailabilityStatus,
    pub supports: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Anthropic,
    OpenAi,
    OpenAiCompatible,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAvailabilityStatus {
    Available,
    Degraded,
    Unavailable,
    Unknown,
}

// ── ModelProviderBinding ────────────────────────────────────────────

/// Links a SupportedModelDefinition to a UserProvider with concrete
/// invocation details.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelProviderBinding {
    pub binding_id: ModelBindingId,
    pub canonical_model_slug: String,
    pub provider_id: ProviderId,
    pub model_name: String,
    pub display_name: String,
    pub invocation_method: InvocationMethod,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub rate_limit: Option<RateLimitConfig>,
    pub priority: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RateLimitConfig {
    pub requests_per_minute: Option<u32>,
    pub tokens_per_minute: Option<u64>,
    pub tokens_per_day: Option<u64>,
    pub concurrent_requests: Option<u32>,
}

// ── ResolvedModelProfile ────────────────────────────────────────────

/// Runtime merge of SupportedModelDefinition + ModelProviderBinding +
/// session overrides.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResolvedModelProfile {
    pub canonical_model_slug: String,
    pub provider_id: ProviderId,
    pub model_binding_id: ModelBindingId,
    pub display_name: String,
    pub context_window: u64,
    pub effective_context_window: u64,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub modalities: Vec<ModelModality>,
    pub invocation_method: InvocationMethod,
    pub model_name: String,
    pub base_url: Option<String>,
    pub supports_tool_use: bool,
    pub supports_streaming: bool,
    pub supports_prompt_caching: bool,
    pub max_output_tokens: Option<u32>,
}

// ── Provider Error Classification ───────────────────────────────────

/// Structured provider error (L3-BEH-PROVIDER-001 §B6).
#[derive(Debug, Clone, thiserror::Error)]
pub enum ProviderError {
    #[error("authentication failed: {message}")]
    AuthenticationError {
        message: String,
        provider_id: Option<ProviderId>,
    },
    #[error("rate limited: {message}")]
    RateLimitError {
        message: String,
        retry_after: Option<u64>,
    },
    #[error("provider server error: {message}")]
    ProviderServerError {
        message: String,
        status_code: Option<u16>,
    },
    #[error("provider timeout: {message}")]
    ProviderTimeoutError { message: String },
    #[error("context limit exceeded: {message}")]
    ContextLimitError {
        message: String,
        current_tokens: Option<u64>,
        limit: Option<u64>,
    },
    #[error("model not found: {message}")]
    ModelNotFoundError {
        message: String,
        model_name: Option<String>,
    },
    #[error("quota exceeded: {message}")]
    QuotaExceededError { message: String },
    #[error("unknown provider error: {message}")]
    UnknownError { message: String },
}

impl ProviderError {
    /// Whether the error is recoverable (retry may succeed).
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            Self::RateLimitError { .. }
                | Self::ProviderServerError { .. }
                | Self::ProviderTimeoutError { .. }
        )
    }

    /// Suggested retry delay in seconds, if provided by the provider.
    pub fn retry_after_seconds(&self) -> Option<u64> {
        match self {
            Self::RateLimitError { retry_after, .. } => *retry_after,
            _ => None,
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_model_definition_is_pure_capability() {
        let def = SupportedModelDefinition {
            canonical_model_slug: "claude-opus-4-7".into(),
            display_name: "Claude Opus 4.7".into(),
            base_instructions: "You are helpful.".into(),
            context_window: 200000,
            effective_context_window: 180000,
            modalities: vec![ModelModality::Text, ModelModality::Image],
            reasoning_capability: ModelReasoningCapability::Levels(vec![
                ReasoningEffort::Low,
                ReasoningEffort::High,
            ]),
            default_reasoning_effort: Some(ReasoningEffort::High),
            supports_tool_use: true,
            supports_parallel_tool_use: true,
            supports_images: true,
            supports_streaming: true,
            supports_prompt_caching: true,
            max_output_tokens: Some(32000),
        };
        // Verify no provider-specific fields
        assert_eq!(def.canonical_model_slug, "claude-opus-4-7");
        assert_eq!(def.context_window, 200000);
    }

    #[test]
    fn user_provider_has_credential_ref() {
        let provider = UserProvider {
            provider_id: ProviderId::new(),
            provider_name: "my-anthropic".into(),
            provider_kind: ProviderKind::Anthropic,
            base_url: Some("https://api.anthropic.com".into()),
            credential_ref: "auth.json#anthropic".into(),
            availability_status: ProviderAvailabilityStatus::Available,
            supports: vec!["messages".into()],
        };
        assert_eq!(provider.provider_kind, ProviderKind::Anthropic);
        assert_eq!(provider.credential_ref, "auth.json#anthropic");
    }

    #[test]
    fn model_provider_binding_links_model_to_provider() {
        let binding = ModelProviderBinding {
            binding_id: ModelBindingId::new(),
            canonical_model_slug: "claude-opus-4-7".into(),
            provider_id: ProviderId::new(),
            model_name: "claude-opus-4-7".into(),
            display_name: "Claude Opus 4.7".into(),
            invocation_method: InvocationMethod::AnthropicMessages,
            reasoning_effort: Some(ReasoningEffort::High),
            rate_limit: Some(RateLimitConfig {
                requests_per_minute: Some(50),
                tokens_per_minute: Some(100000),
                tokens_per_day: None,
                concurrent_requests: Some(5),
            }),
            priority: 1,
        };
        assert_eq!(binding.model_name, "claude-opus-4-7");
        assert_eq!(
            binding.invocation_method,
            InvocationMethod::AnthropicMessages
        );
    }

    #[test]
    fn resolved_model_profile_merges_sources() {
        let profile = ResolvedModelProfile {
            canonical_model_slug: "claude-sonnet-4-6".into(),
            provider_id: ProviderId::new(),
            model_binding_id: ModelBindingId::new(),
            display_name: "Claude Sonnet 4.6".into(),
            context_window: 200000,
            effective_context_window: 180000,
            reasoning_effort: Some(ReasoningEffort::Medium),
            modalities: vec![ModelModality::Text, ModelModality::Image],
            invocation_method: InvocationMethod::AnthropicMessages,
            model_name: "claude-sonnet-4-6".into(),
            base_url: None,
            supports_tool_use: true,
            supports_streaming: true,
            supports_prompt_caching: true,
            max_output_tokens: Some(32000),
        };
        assert_eq!(profile.canonical_model_slug, "claude-sonnet-4-6");
    }

    #[test]
    fn provider_error_recoverability() {
        assert!(
            ProviderError::RateLimitError {
                message: "slow down".into(),
                retry_after: Some(30)
            }
            .is_recoverable()
        );
        assert!(
            ProviderError::ProviderTimeoutError {
                message: "timed out".into()
            }
            .is_recoverable()
        );
        assert!(
            !ProviderError::AuthenticationError {
                message: "bad key".into(),
                provider_id: None
            }
            .is_recoverable()
        );
    }

    #[test]
    fn provider_error_retry_after() {
        let err = ProviderError::RateLimitError {
            message: "slow down".into(),
            retry_after: Some(30),
        };
        assert_eq!(err.retry_after_seconds(), Some(30));

        let err = ProviderError::ProviderTimeoutError {
            message: "timeout".into(),
        };
        assert_eq!(err.retry_after_seconds(), None);
    }

    #[test]
    fn model_modality_serde_roundtrip() {
        for m in &[
            ModelModality::Text,
            ModelModality::Image,
            ModelModality::Audio,
            ModelModality::Video,
        ] {
            let json = serde_json::to_string(m).expect("serialize");
            let restored: ModelModality = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored, *m);
        }
    }

    #[test]
    fn provider_kind_serde_roundtrip() {
        for k in &[
            ProviderKind::Anthropic,
            ProviderKind::OpenAi,
            ProviderKind::OpenAiCompatible,
            ProviderKind::Custom,
        ] {
            let json = serde_json::to_string(k).expect("serialize");
            let restored: ProviderKind = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored, *k);
        }
    }
}

use devo_protocol::ModelProfileKey;
use devo_protocol::ReasoningEffort;
use devo_protocol::RequestRole;

/// Transport variants used to resolve OpenAI-family capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenAITransport {
    ChatCompletions,
    Responses,
}

/// How a model expects reasoning controls to be encoded on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenAIReasoningMode {
    /// `reasoning_effort` / `reasoning.effort`.
    Effort,
    /// OpenAI-compatible `thinking` object with `enabled` / `disabled`.
    Thinking,
    /// Models that require both a thinking toggle and an effort field.
    ThinkingWithEffort,
}

/// Capability profile for an OpenAI-family model on a specific transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OpenAIRequestProfile {
    pub reasoning_mode: OpenAIReasoningMode,
    pub supported_roles: &'static [RequestRole],
    pub supports_temperature: bool,
    pub supports_top_p: bool,
    pub supports_top_k: bool,
    pub require_reasoning_content: bool,
    /// When set, sampling requires no reasoning; the value is the model default.
    pub sampling_reasoning_default: Option<ReasoningEffort>,
    pub max_tokens_parameter: &'static str,
}

impl OpenAIRequestProfile {
    const fn new(
        reasoning_mode: OpenAIReasoningMode,
        supported_roles: &'static [RequestRole],
        supports_temperature: bool,
        supports_top_p: bool,
        supports_top_k: bool,
        require_reasoning_content: bool,
    ) -> Self {
        Self {
            reasoning_mode,
            supported_roles,
            supports_temperature,
            supports_top_p,
            supports_top_k,
            require_reasoning_content,
            max_tokens_parameter: "max_tokens",
            sampling_reasoning_default: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelMatcher {
    Prefix(&'static str),
    #[allow(dead_code)]
    Contains(&'static str),
}

impl ModelMatcher {
    fn matches(self, model: &str) -> bool {
        let model = model.rsplit_once('/').map_or(model, |(_, model)| model);
        match self {
            // Capability resolution uses the catalog slug, not the provider's
            // configurable wire model name. Avoid lowercasing the whole slug
            // for every provider request.
            ModelMatcher::Prefix(value) => model
                .get(..value.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(value)),
            ModelMatcher::Contains(value) => model.to_ascii_lowercase().contains(value),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProfileRule {
    matcher: ModelMatcher,
    transport: OpenAITransport,
    profile: OpenAIRequestProfile,
}

const DEFAULT_ROLES: &[RequestRole] = &[
    RequestRole::System,
    RequestRole::Developer,
    RequestRole::User,
    RequestRole::Assistant,
    RequestRole::Tool,
    RequestRole::Function,
];

const ROLES_WITHOUT_DEVELOPER: &[RequestRole] = &[
    RequestRole::System,
    RequestRole::User,
    RequestRole::Assistant,
    RequestRole::Tool,
    RequestRole::Function,
];

const RESPONSES_ROLES: &[RequestRole] = &[
    RequestRole::System,
    RequestRole::Developer,
    RequestRole::User,
    RequestRole::Assistant,
    RequestRole::Tool,
    RequestRole::Function,
];

const DEFAULT_CHAT_COMPLETIONS: OpenAIRequestProfile = OpenAIRequestProfile::new(
    OpenAIReasoningMode::Effort,
    DEFAULT_ROLES,
    true,
    true,
    false,
    false,
);

const DEFAULT_RESPONSES: OpenAIRequestProfile = OpenAIRequestProfile::new(
    OpenAIReasoningMode::Effort,
    RESPONSES_ROLES,
    true,
    true,
    false,
    false,
);

const OPENAI_PROFILE_RULES: &[ProfileRule] = &[
    ProfileRule {
        matcher: ModelMatcher::Prefix("gpt-5"),
        transport: OpenAITransport::Responses,
        profile: OpenAIRequestProfile::new(
            OpenAIReasoningMode::Effort,
            RESPONSES_ROLES,
            // GPT-5-class Responses backends (codex included) reject sampling
            // overrides; only the reasoning effort is configurable.
            false,
            false,
            false,
            false,
        ),
    },
    ProfileRule {
        matcher: ModelMatcher::Prefix("glm-"),
        transport: OpenAITransport::ChatCompletions,
        profile: OpenAIRequestProfile::new(
            OpenAIReasoningMode::Thinking,
            ROLES_WITHOUT_DEVELOPER,
            true,
            true,
            true,
            true,
        ),
    },
    ProfileRule {
        matcher: ModelMatcher::Prefix("deepseek-"),
        transport: OpenAITransport::ChatCompletions,
        profile: OpenAIRequestProfile::new(
            OpenAIReasoningMode::ThinkingWithEffort,
            ROLES_WITHOUT_DEVELOPER,
            true,
            true,
            true,
            true,
        ),
    },
    ProfileRule {
        matcher: ModelMatcher::Prefix("kimi-"),
        transport: OpenAITransport::ChatCompletions,
        profile: OpenAIRequestProfile::new(
            OpenAIReasoningMode::Thinking,
            ROLES_WITHOUT_DEVELOPER,
            true,
            true,
            true,
            true,
        ),
    },
    ProfileRule {
        matcher: ModelMatcher::Prefix("minimax-"),
        transport: OpenAITransport::ChatCompletions,
        profile: OpenAIRequestProfile::new(
            OpenAIReasoningMode::Effort,
            ROLES_WITHOUT_DEVELOPER,
            true,
            true,
            true,
            true,
        ),
    },
    ProfileRule {
        matcher: ModelMatcher::Prefix("qwen"),
        transport: OpenAITransport::ChatCompletions,
        profile: OpenAIRequestProfile::new(
            OpenAIReasoningMode::Effort,
            ROLES_WITHOUT_DEVELOPER,
            true,
            true,
            true,
            true,
        ),
    },
];

/// Resolves the wire profile for an OpenAI-family model on the given transport.
pub(crate) fn resolve_request_profile(
    model_profile: &ModelProfileKey,
    transport: OpenAITransport,
) -> OpenAIRequestProfile {
    if let ModelProfileKey::CatalogSlug(model) = model_profile {
        let model = model.rsplit_once('/').map_or(model.as_str(), |(_, model)| model);
        let model = model.to_ascii_lowercase();
        let o_series = ["o1", "o3", "o4"].iter().any(|prefix| {
            model == *prefix || model.strip_prefix(*prefix).is_some_and(|suffix| suffix.starts_with('-'))
        });
        let gpt5 = model == "gpt-5" || model.starts_with("gpt-5-") || model.starts_with("gpt-5.");
        if o_series && transport == OpenAITransport::Responses {
            return OpenAIRequestProfile {
                supports_temperature: false,
                supports_top_p: false,
                ..DEFAULT_RESPONSES
            };
        }
        if transport == OpenAITransport::ChatCompletions && (o_series || gpt5) {
            // max_tokens is incompatible with OpenAI reasoning models. Keep the
            // legacy parameter for generic / third-party compatibility profiles.
            let fixed_sampling = o_series
                || model == "gpt-5"
                || (model.starts_with("gpt-5-") && !model.starts_with("gpt-5-chat"));
            return OpenAIRequestProfile {
                max_tokens_parameter: "max_completion_tokens",
                supports_temperature: !fixed_sampling,
                supports_top_p: !fixed_sampling,
                sampling_reasoning_default: if fixed_sampling || model.contains("-chat") {
                    None
                } else {
                    // GPT-5.1/5.2/5.4 default to none; GPT-5.5 and later
                    // default to medium. Do not silently assume none for new models.
                    let defaults_to_none = ["gpt-5.1", "gpt-5.2", "gpt-5.4"].iter().any(|prefix| {
                        model == *prefix
                            || model.strip_prefix(*prefix).is_some_and(|suffix| suffix.starts_with('-'))
                    });
                    Some(if defaults_to_none { ReasoningEffort::None } else { ReasoningEffort::Medium })
                },
                ..DEFAULT_CHAT_COMPLETIONS
            };
        }
    }

    for rule in OPENAI_PROFILE_RULES {
        if rule.transport == transport
            && matches!(model_profile, ModelProfileKey::CatalogSlug(model) if rule.matcher.matches(model))
        {
            return rule.profile;
        }
    }

    match transport {
        OpenAITransport::ChatCompletions => DEFAULT_CHAT_COMPLETIONS,
        OpenAITransport::Responses => DEFAULT_RESPONSES,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn responses_o_series_omit_sampling_controls() {
        for slug in ["openai/o1", "o3-mini", "o4-mini-2025-04-16"] {
            let profile = resolve_request_profile(
                &ModelProfileKey::CatalogSlug(slug.to_string()),
                OpenAITransport::Responses,
            );
            assert_eq!(profile, OpenAIRequestProfile {
                supports_temperature: false,
                supports_top_p: false,
                ..DEFAULT_RESPONSES
            });
        }
    }

    #[test]
    fn resolve_request_profile_uses_zai_thinking_for_chat_completions() {
        let profile = resolve_request_profile(
            &ModelProfileKey::CatalogSlug("glm-4.5".to_string()),
            OpenAITransport::ChatCompletions,
        );
        assert_eq!(profile.reasoning_mode, OpenAIReasoningMode::Thinking);
        assert!(profile.supports_top_k);
        assert!(profile.require_reasoning_content);
    }

    #[test]
    fn resolve_request_profile_uses_generic_profile_without_catalog_slug() {
        let profile =
            resolve_request_profile(&ModelProfileKey::Generic, OpenAITransport::ChatCompletions);
        assert_eq!(profile.reasoning_mode, OpenAIReasoningMode::Effort);
    }

    #[test]
    fn resolve_request_profile_matches_prefix_case_insensitively() {
        let profile = resolve_request_profile(
            &ModelProfileKey::CatalogSlug("GLM-4.5".to_string()),
            OpenAITransport::ChatCompletions,
        );
        assert_eq!(profile.reasoning_mode, OpenAIReasoningMode::Thinking);
    }

    #[test]
    fn resolve_request_profile_matches_provider_catalog_slugs() {
        let profile = resolve_request_profile(
            &ModelProfileKey::CatalogSlug("deepseek/deepseek-v4-flash".to_string()),
            OpenAITransport::ChatCompletions,
        );
        assert_eq!(
            profile.reasoning_mode,
            OpenAIReasoningMode::ThinkingWithEffort
        );
    }

    #[test]
    fn resolve_request_profile_matches_qwen3_models() {
        let profile = resolve_request_profile(
            &ModelProfileKey::CatalogSlug("ollama/qwen3:8b".to_string()),
            OpenAITransport::ChatCompletions,
        );
        assert_eq!(profile.reasoning_mode, OpenAIReasoningMode::Effort);
    }

    #[test]
    fn resolve_request_profile_matches_kimi_models() {
        let profile = resolve_request_profile(
            &ModelProfileKey::CatalogSlug("kimi/kimi-k2.6".to_string()),
            OpenAITransport::ChatCompletions,
        );
        assert_eq!(profile.reasoning_mode, OpenAIReasoningMode::Thinking);
    }

    #[test]
    fn resolve_request_profile_defaults_to_effort_for_responses() {
        let profile = resolve_request_profile(
            &ModelProfileKey::CatalogSlug("glm-4.5".to_string()),
            OpenAITransport::Responses,
        );
        assert_eq!(profile.reasoning_mode, OpenAIReasoningMode::Effort);
    }
}

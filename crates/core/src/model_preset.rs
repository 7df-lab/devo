//! Raw model preset types used to load the builtin catalog.
//!
//! Main focus:
//! - deserialize bundled model definitions from `models.json`
//! - preserve JSON compatibility and catalog-only metadata such as priority and API-config flags
//! - convert raw presets into runtime `devo_protocol::Model` values
//!
//! Design:
//! - `ModelPreset` is intentionally a core-only type because it exists to support catalog loading
//! - serde adapters and legacy field aliases live here so they do not leak into the runtime model
//! - conversion into `Model` is the handoff point from config data to executable runtime data
//!
//! Boundary:
//! - this module should not act as the runtime model API seen by server, client, or query code
//! - turn execution should consume `Model`, not `ModelPreset`
//! - loading policy and catalog access live in `model_catalog.rs`; this file only defines the raw shape
//!
use devo_protocol::InputModality;
use devo_protocol::Model;
use devo_protocol::ProviderWireApi;
use devo_protocol::ReasoningCapability;
use devo_protocol::ReasoningEffort;
use devo_protocol::ReasoningImplementation;
use devo_protocol::TruncationPolicyConfig;
use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
/// Raw catalog preset loaded from the bundled model JSON.
pub struct ModelPreset {
    /// Stable model identifier used in config and requests. such as `claude-sonnet-20250425`
    pub slug: String,
    /// Human-readable display name shown in the UI. such as `claude-sonnet-4.6`
    pub display_name: String,
    /// Provider selection that serves this model.
    pub provider: ProviderWireApi,
    /// Optional short description of the model.
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub description: Option<String>,
    /// Reasoning control available for this model.
    #[serde(
        default = "default_reasoning_capability",
        alias = "thinking_capability",
        deserialize_with = "deserialize_reasoning_capability"
    )]
    pub reasoning_capability: ReasoningCapability,
    /// Legacy list of supported reasoning levels used by some bundled presets.
    #[serde(default, alias = "supported_reasoning_levels")]
    pub supported_reasoning_levels: Vec<ReasoningEffort>,
    /// Default reasoning effort selected for the model when no levels are exposed.
    #[serde(
        default = "default_reasoning_effort",
        alias = "default_reasoning_level",
        deserialize_with = "deserialize_reasoning_effort_option"
    )]
    pub default_reasoning_effort: Option<ReasoningEffort>,
    /// How the selected reasoning effort should be applied to requests.
    #[serde(default, alias = "thinking_implementation")]
    pub reasoning_implementation: Option<ReasoningImplementation>,
    /// Base system instructions bundled with the model.
    pub base_instructions: String,
    /// Maximum context window in tokens.
    #[serde(default = "default_context_window")]
    pub context_window: u32,
    /// Percentage of the context window treated as effectively usable.
    pub effective_context_window_percent: Option<u8>,
    /// Policy used when truncating content for requests.
    #[serde(
        default,
        deserialize_with = "devo_protocol::deserialize_truncation_policy_config"
    )]
    pub truncation_policy: TruncationPolicyConfig,
    /// Input types accepted by the model.
    #[serde(default = "default_input_modalities")]
    pub input_modalities: Vec<InputModality>,
    /// Whether the model supports original-resolution image detail.
    pub supports_image_detail_original: bool,
    /// Grouping label used to organize models by vendor or family.
    pub channel: Option<String>,
    /// Whether the user configured API access for this model.
    #[serde(rename = "supported_in_api")]
    pub api_configured: bool,
    /// Default temperature to use when the model does not override it.
    pub temperature: Option<f64>,
    /// Default nucleus sampling value to use when the model does not override it.
    pub top_p: Option<f64>,
    /// Default top-k sampling value to use when the model does not override it.
    pub top_k: Option<f64>,
    /// Default maximum token limit for responses from this model.
    pub max_tokens: Option<u32>,
    /// Relative priority used when choosing a default visible model.
    pub priority: i32,
}

impl Default for ModelPreset {
    fn default() -> Self {
        Self {
            slug: String::new(),
            display_name: String::new(),
            provider: ProviderWireApi::OpenAIChatCompletions,
            description: None,
            reasoning_capability: ReasoningCapability::Unsupported,
            supported_reasoning_levels: Vec::new(),
            default_reasoning_effort: Some(ReasoningEffort::default()),
            reasoning_implementation: None,
            base_instructions: String::new(),
            context_window: 200_000,
            effective_context_window_percent: None,
            truncation_policy: TruncationPolicyConfig::default(),
            input_modalities: vec![InputModality::default()],
            supports_image_detail_original: false,
            channel: None,
            api_configured: false,
            temperature: None,
            top_p: None,
            top_k: None,
            max_tokens: None,
            priority: 0,
        }
    }
}

impl From<ModelPreset> for Model {
    fn from(value: ModelPreset) -> Self {
        let supported_reasoning_levels = value.supported_reasoning_levels;
        let default_reasoning_effort = value.default_reasoning_effort;

        // Legacy presets express "toggle with selectable levels" as a plain
        // toggle plus a non-empty level list. Move that list into the runtime
        // shape once; catalog loading can convert many presets at startup.
        let reasoning_capability = match value.reasoning_capability {
            ReasoningCapability::Toggle if !supported_reasoning_levels.is_empty() => {
                ReasoningCapability::ToggleWithLevels(supported_reasoning_levels)
            }
            capability => capability,
        };
        let default_reasoning_effort = match &reasoning_capability {
            ReasoningCapability::ToggleWithLevels(levels) => {
                default_reasoning_effort.or_else(|| levels.first().copied())
            }
            _ => default_reasoning_effort,
        };

        Self {
            slug: value.slug,
            display_name: value.display_name,
            provider: value.provider,
            description: value.description,
            reasoning_capability,
            default_reasoning_effort,
            reasoning_implementation: value.reasoning_implementation,
            base_instructions: value.base_instructions,
            context_window: value.context_window,
            effective_context_window_percent: value.effective_context_window_percent,
            truncation_policy: value.truncation_policy,
            input_modalities: value.input_modalities,
            supports_image_detail_original: value.supports_image_detail_original,
            channel: value.channel,
            temperature: value.temperature,
            top_p: value.top_p,
            top_k: value.top_k,
            max_tokens: value.max_tokens,
        }
    }
}

fn default_reasoning_effort() -> Option<ReasoningEffort> {
    Some(ReasoningEffort::default())
}

fn default_context_window() -> u32 {
    200_000
}

fn default_input_modalities() -> Vec<InputModality> {
    vec![InputModality::Text, InputModality::Image]
}

fn default_reasoning_capability() -> ReasoningCapability {
    ReasoningCapability::Unsupported
}

fn deserialize_optional_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    Ok(value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(value)
        }
    }))
}

fn deserialize_reasoning_effort_option<'de, D>(
    deserializer: D,
) -> Result<Option<ReasoningEffort>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Null => Ok(default_reasoning_effort()),
        serde_json::Value::String(text) if text.trim().is_empty() => Ok(default_reasoning_effort()),
        other => serde_json::from_value(other)
            .map(Some)
            .map_err(serde::de::Error::custom),
    }
}

fn deserialize_reasoning_capability<'de, D>(
    deserializer: D,
) -> Result<ReasoningCapability, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Null => Ok(default_reasoning_capability()),
        serde_json::Value::String(text) if text.trim().is_empty() => {
            Ok(default_reasoning_capability())
        }
        other => serde_json::from_value(other).map_err(serde::de::Error::custom),
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn conversion_promotes_legacy_toggle_levels() {
        let preset = ModelPreset {
            slug: "legacy-toggle".to_string(),
            display_name: "Legacy Toggle".to_string(),
            reasoning_capability: ReasoningCapability::Toggle,
            supported_reasoning_levels: vec![ReasoningEffort::High, ReasoningEffort::Max],
            default_reasoning_effort: None,
            ..ModelPreset::default()
        };

        let model = Model::from(preset);

        assert_eq!(
            model,
            Model {
                slug: "legacy-toggle".to_string(),
                display_name: "Legacy Toggle".to_string(),
                reasoning_capability: ReasoningCapability::ToggleWithLevels(vec![
                    ReasoningEffort::High,
                    ReasoningEffort::Max,
                ]),
                default_reasoning_effort: Some(ReasoningEffort::High),
                ..Model::default()
            }
        );
    }

    #[test]
    fn model_preset_reads_legacy_reasoning_keys() {
        let preset: ModelPreset = serde_json::from_value(serde_json::json!({
            "slug": "legacy",
            "display_name": "Legacy",
            "provider": "openai_chat_completions",
            "thinking_capability": "toggle",
            "thinking_implementation": "request_parameter",
            "base_instructions": "",
            "supported_in_api": true
        }))
        .expect("deserialize legacy preset");

        assert_eq!(preset.reasoning_capability, ReasoningCapability::Toggle);
        assert_eq!(
            preset.reasoning_implementation,
            Some(ReasoningImplementation::RequestParameter)
        );
    }
}

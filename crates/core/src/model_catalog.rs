use clawcr_provider::ProviderFamily;
use serde::Deserialize;

use crate::{
    InputModality, ModelCatalog, ModelPreset, ModelPresetError, ReasoningEffort,
    ThinkingCapability, TruncationPolicyConfig,
};

const DEFAULT_BASE_INSTRUCTIONS: &str = include_str!("../default_base_instructions.txt");

/// Filesystem-independent loader for the built-in model catalog bundled with the binary.
#[derive(Debug, Clone, Default)]
pub struct BuiltinModelCatalog {
    models: Vec<ModelPreset>,
}

impl BuiltinModelCatalog {
    /// Loads the built-in catalog from `crates/core/models.json`.
    pub fn load() -> Result<Self, BuiltinModelCatalogError> {
        Ok(Self {
            models: load_builtin_models()?,
        })
    }

    /// Creates a catalog from an already-loaded model list.
    pub fn new(models: Vec<ModelPreset>) -> Self {
        Self { models }
    }

    /// Returns the loaded models by value.
    pub fn into_inner(self) -> Vec<ModelPreset> {
        self.models
    }
}

impl ModelCatalog for BuiltinModelCatalog {
    fn list_visible(&self) -> Vec<&ModelPreset> {
        self.models.iter().collect()
    }

    fn get(&self, slug: &str) -> Option<&ModelPreset> {
        self.models.iter().find(|model| model.slug == slug)
    }

    fn resolve_for_turn(&self, requested: Option<&str>) -> Result<&ModelPreset, ModelPresetError> {
        if let Some(slug) = requested {
            return self
                .get(slug)
                .ok_or_else(|| ModelPresetError::ModelNotFound {
                    slug: slug.to_string(),
                });
        }

        self.list_visible()
            .into_iter()
            .max_by_key(|model| model.priority)
            .ok_or(ModelPresetError::NoVisibleModels)
    }
}

/// Loads the built-in model list bundled with the crate.
pub fn load_builtin_models() -> Result<Vec<ModelPreset>, BuiltinModelCatalogError> {
    let raw_models: Vec<RawBuiltinModelPreset> =
        serde_json::from_str(include_str!("../models.json"))?;
    Ok(raw_models
        .into_iter()
        .map(RawBuiltinModelPreset::into_model)
        .collect())
}

/// Returns the shared fallback base instructions used when a model has no catalog entry.
pub fn default_base_instructions() -> &'static str {
    DEFAULT_BASE_INSTRUCTIONS
}

/// Errors produced while loading the builtin catalog.
#[derive(Debug, thiserror::Error)]
pub enum BuiltinModelCatalogError {
    /// Parsing the bundled JSON file failed.
    #[error("failed to parse builtin model catalog: {0}")]
    Parse(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Deserialize)]
struct RawBuiltinModelPreset {
    slug: String,
    display_name: String,
    provider: ProviderFamily,
    #[serde(default)]
    description: String,
    #[serde(
        default,
        alias = "default_reasoning_level",
        deserialize_with = "deserialize_reasoning_effort"
    )]
    default_reasoning_effort: ReasoningEffort,
    #[serde(default, alias = "supported_reasoning_levels")]
    supported_reasoning_efforts: Vec<ReasoningEffort>,
    #[serde(default)]
    thinking_capability: Option<RawThinkingCapability>,
    base_instructions: String,
    #[serde(default)]
    context_window: Option<u32>,
    #[serde(default)]
    effective_context_window_percent: Option<u8>,
    #[serde(default, deserialize_with = "deserialize_truncation_policy")]
    truncation_policy: TruncationPolicyConfig,
    #[serde(default)]
    input_modalities: Vec<InputModality>,
    #[serde(default)]
    supports_image_detail_original: bool,
    #[serde(default, alias = "supported_in_api")]
    api_configured: bool,
    #[serde(default)]
    priority: i32,
}

impl RawBuiltinModelPreset {
    fn into_model(self) -> ModelPreset {
        let supported_reasoning_efforts = if self.supported_reasoning_efforts.is_empty() {
            vec![self.default_reasoning_effort]
        } else {
            self.supported_reasoning_efforts
        };
        let thinking_capability = match self.thinking_capability.unwrap_or_default() {
            RawThinkingCapability::Levels => {
                ThinkingCapability::Levels(supported_reasoning_efforts.clone())
            }
            RawThinkingCapability::Toggle => ThinkingCapability::Toggle,
            RawThinkingCapability::Disabled => ThinkingCapability::Disabled,
        };
        let mut model = ModelPreset::default();
        model.slug = self.slug;
        model.display_name = self.display_name;
        model.provider = self.provider;
        model.description = if self.description.trim().is_empty() {
            None
        } else {
            Some(self.description)
        };
        model.default_reasoning_effort = Some(self.default_reasoning_effort);
        model.thinking_capability = thinking_capability;
        model.base_instructions = self.base_instructions;
        model.context_window = self.context_window.unwrap_or(model.context_window);
        model.effective_context_window_percent = self.effective_context_window_percent;
        model.truncation_policy = self.truncation_policy;
        model.input_modalities = if self.input_modalities.is_empty() {
            vec![InputModality::Text]
        } else {
            self.input_modalities
        };
        model.supports_image_detail_original = self.supports_image_detail_original;
        model.api_configured = self.api_configured;
        model.priority = self.priority;
        model
    }
}

fn deserialize_reasoning_effort<'de, D>(deserializer: D) -> Result<ReasoningEffort, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::String(text) if text.trim().is_empty() => Ok(ReasoningEffort::default()),
        other => serde_json::from_value(other).map_err(serde::de::Error::custom),
    }
}

fn deserialize_truncation_policy<'de, D>(
    deserializer: D,
) -> Result<TruncationPolicyConfig, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Null => Ok(TruncationPolicyConfig::default()),
        serde_json::Value::String(text) if text.trim().is_empty() => {
            Ok(TruncationPolicyConfig::default())
        }
        other @ serde_json::Value::Object(_) => {
            serde_json::from_value(other).map_err(serde::de::Error::custom)
        }
        other => Err(serde::de::Error::custom(format!(
            "expected truncation policy object or empty string, got {other}"
        ))),
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
enum RawThinkingCapability {
    #[default]
    Levels,
    Toggle,
    Disabled,
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::{BuiltinModelCatalog, default_base_instructions, load_builtin_models};
    use crate::ModelCatalog;

    #[test]
    fn builtin_models_load_from_bundled_json() {
        let models = load_builtin_models().expect("load builtin models");
        assert!(!models.is_empty());
        assert_eq!(models[0].slug, "qwen3-coder-next");
        assert!(!models[0].base_instructions.is_empty());
    }

    #[test]
    fn builtin_catalog_resolves_visible_defaults() {
        let catalog = BuiltinModelCatalog::load().expect("load catalog");
        let model = catalog.resolve_for_turn(None).expect("resolve default");
        assert!(!model.slug.is_empty());
    }

    #[test]
    fn default_base_instructions_are_available() {
        assert!(!default_base_instructions().trim().is_empty());
    }
}

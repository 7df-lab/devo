use serde::Deserialize;

use crate::{
    InputModality, ModelCatalog, ModelConfig, ModelConfigError, ModelVisibility, ProviderKind,
    ReasoningLevel, TruncationPolicyConfig,
};

/// Filesystem-independent loader for the built-in model catalog bundled with the binary.
#[derive(Debug, Clone, Default)]
pub struct BuiltinModelCatalog {
    models: Vec<ModelConfig>,
}

impl BuiltinModelCatalog {
    /// Loads the built-in catalog from `crates/core/models.json`.
    pub fn load() -> Result<Self, BuiltinModelCatalogError> {
        Ok(Self {
            models: load_builtin_models()?,
        })
    }

    /// Creates a catalog from an already-loaded model list.
    pub fn new(models: Vec<ModelConfig>) -> Self {
        Self { models }
    }

    /// Returns the loaded models by value.
    pub fn into_inner(self) -> Vec<ModelConfig> {
        self.models
    }
}

impl ModelCatalog for BuiltinModelCatalog {
    fn list_visible(&self) -> Vec<&ModelConfig> {
        self.models
            .iter()
            .filter(|model| model.visibility == ModelVisibility::Visible)
            .collect()
    }

    fn get(&self, slug: &str) -> Option<&ModelConfig> {
        self.models.iter().find(|model| model.slug == slug)
    }

    fn resolve_for_turn(&self, requested: Option<&str>) -> Result<&ModelConfig, ModelConfigError> {
        if let Some(slug) = requested {
            return self
                .get(slug)
                .ok_or_else(|| ModelConfigError::ModelNotFound {
                    slug: slug.to_string(),
                });
        }

        self.list_visible()
            .into_iter()
            .max_by_key(|model| model.priority)
            .ok_or(ModelConfigError::NoVisibleModels)
    }
}

/// Loads the built-in model list bundled with the crate.
pub fn load_builtin_models() -> Result<Vec<ModelConfig>, BuiltinModelCatalogError> {
    let raw_models: Vec<RawBuiltinModelConfig> =
        serde_json::from_str(include_str!("../models.json"))?;
    Ok(raw_models
        .into_iter()
        .map(RawBuiltinModelConfig::into_model)
        .collect())
}

/// Errors produced while loading the builtin catalog.
#[derive(Debug, thiserror::Error)]
pub enum BuiltinModelCatalogError {
    /// Parsing the bundled JSON file failed.
    #[error("failed to parse builtin model catalog: {0}")]
    Parse(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Deserialize)]
struct RawBuiltinModelConfig {
    slug: String,
    display_name: String,
    provider: ProviderKind,
    #[serde(default)]
    description: String,
    #[serde(default, deserialize_with = "deserialize_reasoning_level")]
    default_reasoning_level: ReasoningLevel,
    #[serde(default)]
    supported_reasoning_levels: Vec<ReasoningLevel>,
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
    #[serde(default)]
    visibility: ModelVisibility,
    #[serde(default)]
    supported_in_api: bool,
    #[serde(default)]
    priority: i32,
}

impl RawBuiltinModelConfig {
    fn into_model(self) -> ModelConfig {
        let mut model = ModelConfig::default();
        model.slug = self.slug;
        model.display_name = self.display_name;
        model.provider = self.provider;
        model.description = if self.description.trim().is_empty() {
            None
        } else {
            Some(self.description)
        };
        model.default_reasoning_level = self.default_reasoning_level;
        model.supported_reasoning_levels = if self.supported_reasoning_levels.is_empty() {
            vec![model.default_reasoning_level.clone()]
        } else {
            self.supported_reasoning_levels
        };
        model.base_instructions = self.base_instructions;
        model.context_window = self.context_window.unwrap_or(model.context_window);
        model.effective_context_window_percent = self
            .effective_context_window_percent
            .unwrap_or(model.effective_context_window_percent);
        model.truncation_policy = self.truncation_policy;
        model.input_modalities = if self.input_modalities.is_empty() {
            vec![InputModality::Text]
        } else {
            self.input_modalities
        };
        model.supports_image_detail_original = self.supports_image_detail_original;
        model.visibility = self.visibility;
        model.supported_in_api = self.supported_in_api;
        model.priority = self.priority;
        model
    }
}

fn deserialize_reasoning_level<'de, D>(deserializer: D) -> Result<ReasoningLevel, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::String(text) if text.trim().is_empty() => Ok(ReasoningLevel::default()),
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

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::{load_builtin_models, BuiltinModelCatalog};
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
}

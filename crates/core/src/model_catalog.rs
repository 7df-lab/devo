//! Builtin model catalog loading and resolution for core.
//!
//! The embedded `providers.json` asset is the canonical provider/model
//! directory. Provider and model identity are resolved from the same map.
use std::collections::BTreeMap;

use crate::{
    InputModality, Model, ModelCatalog, ModelEffortVariant, ModelError, ProviderInfo,
    ProviderModelInfo, ProviderModelVariant, ProviderWireApi, ReasoningCapability,
};
use devo_config::{ModelOverrideConfig, ProviderConfigFile, ProviderModelConfig, model_reference};

const BUILTIN_PROVIDERS_JSON: &str = include_str!("../providers.json");
const DEFAULT_BASE_INSTRUCTIONS: &str = include_str!("../default_base_instructions.txt");

/// Returns the shared fallback base instructions used when a catalog model
/// omits `base_instructions`, or when a custom model has no instructions.
pub fn default_base_instructions() -> &'static str {
    DEFAULT_BASE_INSTRUCTIONS
}

/// A catalog resolved from embedded presets and configuration overrides.
#[derive(Debug, Clone, Default)]
pub struct PresetModelCatalog {
    models: Vec<Model>,
    providers: Vec<ProviderInfo>,
    provider_models: BTreeMap<String, BTreeMap<String, ProviderModelInfo>>,
    builtin_provider_ids: Vec<String>,
}

impl PresetModelCatalog {
    /// Loads the embedded provider/model directory without user overlays.
    pub fn load() -> Result<Self, PresetModelCatalogError> {
        Self::load_from_provider_config(&ProviderConfigFile::default())
    }

    /// Loads the embedded provider/model directory and overlays user-defined
    /// providers and models on top of it.
    pub fn load_from_provider_config(
        provider_config: &ProviderConfigFile,
    ) -> Result<Self, PresetModelCatalogError> {
        Self::load_from_provider_config_with_home(provider_config, &BTreeMap::new(), None)
    }

    /// Loads the embedded catalog, merges the user provider file, then applies
    /// `[model.<slug>]` overlays onto matching builtin or user models.
    pub fn load_from_provider_config_with_overrides(
        provider_config: &ProviderConfigFile,
        model_overrides: &BTreeMap<String, ModelOverrideConfig>,
    ) -> Result<Self, PresetModelCatalogError> {
        Self::load_from_provider_config_with_home(provider_config, model_overrides, None)
    }

    /// Like [`Self::load_from_provider_config_with_overrides`], and also merges
    /// a cached models.dev overlay from `$DEVO_HOME/cache/` when `home_dir` is
    /// provided.
    pub fn load_from_provider_config_with_home(
        provider_config: &ProviderConfigFile,
        model_overrides: &BTreeMap<String, ModelOverrideConfig>,
        home_dir: Option<&std::path::Path>,
    ) -> Result<Self, PresetModelCatalogError> {
        let mut directory = load_base_provider_config(home_dir)?;
        let builtin_provider_ids = directory.providers.keys().cloned().collect();
        directory.merge_overlay(provider_config.clone());
        directory.apply_model_overrides(model_overrides);
        let providers = directory
            .providers
            .iter()
            .map(|(provider_id, provider)| provider_info_from_config(provider_id, provider))
            .collect();
        let referenced_models = [directory.model.clone(), directory.small_model.clone()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        for model_ref in referenced_models {
            let Some((provider_id, requested_model_id)) = model_ref.split_once('/') else {
                continue;
            };
            let Some(provider) = directory.providers.get(provider_id) else {
                continue;
            };
            let model_id = if provider.models.contains_key(requested_model_id) {
                requested_model_id
            } else if let Some((model_id, variant_id)) = requested_model_id.rsplit_once('/')
                && provider
                    .models
                    .get(model_id)
                    .is_some_and(|model| model.variants.contains_key(variant_id))
            {
                model_id
            } else {
                requested_model_id
            };
            if let Some(provider) = directory.providers.get_mut(provider_id) {
                provider.models.entry(model_id.to_string()).or_default();
            }
        }

        let mut models = directory
            .providers
            .iter()
            .flat_map(|(provider_id, provider)| {
                let provider_wire_api = provider
                    .wire_api
                    .unwrap_or(ProviderWireApi::OpenAIChatCompletions);
                provider.models.iter().filter_map(move |(model_id, model)| {
                    if provider.enabled == Some(false) || model.enabled == Some(false) {
                        return None;
                    }
                    Some((
                        model.priority.unwrap_or(0),
                        model_from_provider_config(provider_id, model_id, model, provider_wire_api),
                    ))
                })
            })
            .collect::<Vec<_>>();
        let provider_models = directory
            .providers
            .iter()
            .map(|(provider_id, provider)| {
                let provider_wire_api = provider
                    .wire_api
                    .unwrap_or(ProviderWireApi::OpenAIChatCompletions);
                // Include disabled models so settings UIs can show them with
                // enabled=false. Visible turn selection still uses `models`
                // above, which filters disabled entries out.
                let models = provider
                    .models
                    .iter()
                    .map(|(model_id, model)| {
                        (
                            model_id.clone(),
                            provider_model_info_from_config(model, provider_wire_api),
                        )
                    })
                    .collect();
                (provider_id.clone(), models)
            })
            .collect();
        models.sort_by_key(|left| std::cmp::Reverse(left.0));
        let mut models = models
            .into_iter()
            .map(|(_, model)| model)
            .collect::<Vec<_>>();

        if let Some(default_model) = directory.model.as_deref()
            && let Some(index) = models.iter().position(|model| model.slug == default_model)
        {
            let model = models.remove(index);
            models.insert(0, model);
        }

        Ok(Self {
            models,
            providers,
            provider_models,
            builtin_provider_ids,
        })
    }

    /// Creates a catalog from an already-loaded model list.
    pub fn new(models: Vec<Model>) -> Self {
        Self {
            models,
            providers: Vec::new(),
            provider_models: BTreeMap::new(),
            builtin_provider_ids: Vec::new(),
        }
    }

    /// Returns the loaded models by value.
    pub fn into_inner(self) -> Vec<Model> {
        self.models
    }
}

impl ModelCatalog for PresetModelCatalog {
    fn list_visible(&self) -> Vec<&Model> {
        self.models.iter().collect()
    }

    fn list_providers(&self) -> Vec<ProviderInfo> {
        self.providers.clone()
    }

    fn list_template_provider_ids(&self) -> Vec<String> {
        self.builtin_provider_ids.clone()
    }

    fn list_provider_models(&self, provider_id: &str) -> BTreeMap<String, ProviderModelInfo> {
        self.provider_models
            .get(provider_id)
            .cloned()
            .unwrap_or_default()
    }

    fn get(&self, slug: &str) -> Option<&Model> {
        self.models.iter().find(|model| model.slug == slug)
    }

    /// Resolves an explicit requested slug, or falls back to the first visible preset model.
    fn resolve_for_turn(&self, requested: Option<&str>) -> Result<&Model, ModelError> {
        if let Some(slug) = requested {
            return self.get(slug).ok_or_else(|| ModelError::ModelNotFound {
                slug: slug.to_string(),
            });
        }

        self.list_visible()
            .into_iter()
            .next()
            .ok_or(ModelError::NoVisibleModels)
    }
}

fn load_builtin_provider_config() -> Result<ProviderConfigFile, PresetModelCatalogError> {
    serde_json::from_str(BUILTIN_PROVIDERS_JSON).map_err(Into::into)
}

fn load_base_provider_config(
    home_dir: Option<&std::path::Path>,
) -> Result<ProviderConfigFile, PresetModelCatalogError> {
    let mut directory = load_builtin_provider_config()?;
    if let Some(home_dir) = home_dir {
        match crate::load_remote_catalog_overlay(home_dir) {
            Ok(Some(remote)) => directory.merge_overlay(remote),
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "failed to load cached models.dev catalog overlay"
                );
            }
        }
    }
    Ok(directory)
}

/// Returns the embedded built-in provider directory (no user overlays).
pub fn builtin_provider_config() -> Result<ProviderConfigFile, PresetModelCatalogError> {
    load_builtin_provider_config()
}

/// Builds the effective provider catalog: embedded builtins merged with a
/// sparse user/project overlay (sparse overlay semantics).
///
/// Prefer this for model resolution and routing. Persist/edit paths should
/// keep reading the sparse overlay alone.
pub fn effective_provider_catalog(
    overlay: &ProviderConfigFile,
) -> Result<ProviderConfigFile, PresetModelCatalogError> {
    effective_provider_catalog_with_home(overlay, None)
}

/// Like [`effective_provider_catalog`], including a cached models.dev overlay
/// from `$DEVO_HOME/cache/` when `home_dir` is set.
pub fn effective_provider_catalog_with_home(
    overlay: &ProviderConfigFile,
    home_dir: Option<&std::path::Path>,
) -> Result<ProviderConfigFile, PresetModelCatalogError> {
    let mut directory = load_base_provider_config(home_dir)?;
    directory.merge_overlay(overlay.clone());
    // Session defaults live on the overlay / config.toml projection.
    if directory.model.is_none() {
        directory.model = overlay.model.clone();
    }
    if directory.small_model.is_none() {
        directory.small_model = overlay.small_model.clone();
    }
    if directory.reasoning_effort.is_none() {
        directory.reasoning_effort = overlay.reasoning_effort.clone();
    }
    Ok(directory)
}

/// Rewrites user `providers.json` so built-in Connections store sparse overlays
/// (sparse overlay semantics) and credential ids are provider-keyed.
pub fn migrate_user_provider_catalog_overlays(
    user_config_dir: &std::path::Path,
) -> anyhow::Result<bool> {
    use crate::{
        PROVIDER_CONFIG_FILE_NAME, provider_id_from_credential_id, read_provider_catalog_config,
        sparsify_provider_entry_against_builtin, write_provider_catalog_config,
    };

    let path = user_config_dir.join(PROVIDER_CONFIG_FILE_NAME);
    if !path.exists() {
        return Ok(false);
    }
    let mut config = read_provider_catalog_config(&path)?;
    let builtin = load_builtin_provider_config()?;
    let mut changed = false;
    for (provider_id, entry) in config.providers.iter_mut() {
        if let Some(credential) = entry.credential.clone() {
            let migrated = provider_id_from_credential_id(&credential);
            if migrated != credential {
                entry.credential = Some(migrated);
                changed = true;
            }
        }
        let Some(baseline) = builtin.providers.get(provider_id) else {
            continue;
        };
        let before = entry.clone();
        sparsify_provider_entry_against_builtin(entry, baseline);
        if *entry != before {
            changed = true;
        }
    }
    if changed {
        write_provider_catalog_config(&path, &config)?;
    }
    Ok(changed)
}

fn model_from_provider_config(
    provider_id: &str,
    model_id: &str,
    config: &ProviderModelConfig,
    provider_wire_api: ProviderWireApi,
) -> Model {
    let mut config = config.clone();
    config.migrate_reasoning_implementation_into_variants();
    Model {
        slug: model_reference(provider_id, model_id),
        display_name: config.name.clone().unwrap_or_else(|| model_id.to_string()),
        provider: config.wire_api.unwrap_or(provider_wire_api),
        reasoning_capability: config
            .reasoning_capability
            .clone()
            .unwrap_or(ReasoningCapability::Unsupported),
        reasoning: config.reasoning,
        thinking_level_map: config.thinking_level_map.clone(),
        default_reasoning_effort: config.default_reasoning_effort,
        default_reasoning_selection: config.default_reasoning_selection.clone(),
        reasoning_implementation: config.reasoning_implementation.clone(),
        catalog_variants: config
            .variants
            .iter()
            .map(|(variant_id, variant)| {
                (
                    variant_id.clone(),
                    ModelEffortVariant {
                        request_model: variant.request_model.clone(),
                        disabled: variant.disabled,
                    },
                )
            })
            .collect(),
        base_instructions: config
            .base_instructions
            .clone()
            .unwrap_or_else(|| default_base_instructions().to_string()),
        context_window: config.context_window.unwrap_or(200_000),
        effective_context_window_percent: config.effective_context_window_percent,
        truncation_policy: config.truncation_policy.unwrap_or_default(),
        input_modalities: config
            .input_modalities
            .clone()
            .unwrap_or_else(|| vec![InputModality::Text]),
        supports_image_detail_original: config.supports_image_detail_original.unwrap_or(false),
        channel: config.channel.clone(),
        temperature: config.temperature,
        top_p: config.top_p,
        top_k: config.top_k,
        max_tokens: config.max_tokens,
        ..Model::default()
    }
}

fn provider_info_from_config(
    provider_id: &str,
    config: &devo_config::ProviderConfigEntry,
) -> ProviderInfo {
    let wire_api = config
        .wire_api
        .unwrap_or(ProviderWireApi::OpenAIChatCompletions);
    ProviderInfo {
        id: provider_id.to_string(),
        name: config
            .name
            .clone()
            .unwrap_or_else(|| provider_id.to_string()),
        description: config.description.clone(),
        base_url: config.base_url.clone(),
        credential: config.credential.clone(),
        headers: config.headers.clone().unwrap_or_default(),
        options: config.options.clone(),
        request: config.request.clone(),
        compat: config.compat.clone(),
        wire_apis: vec![wire_api],
        model_overrides: config
            .model_overrides
            .iter()
            .map(|(model_id, model)| {
                (
                    model_id.clone(),
                    provider_model_info_from_config(model, wire_api),
                )
            })
            .collect(),
        models: BTreeMap::new(),
        enabled: config.enabled.unwrap_or(true),
    }
}

fn provider_model_info_from_config(
    config: &ProviderModelConfig,
    provider_wire_api: ProviderWireApi,
) -> ProviderModelInfo {
    let capability = config
        .reasoning_capability
        .clone()
        .unwrap_or(ReasoningCapability::Unsupported);
    let (reasoning, thinking_level_map, _) = devo_protocol::resolve_thinking_fields_for_model_info(
        &capability,
        config.reasoning,
        config.thinking_level_map.as_ref(),
    );
    let project_thinking = config.reasoning.is_some()
        || config
            .thinking_level_map
            .as_ref()
            .is_some_and(|map| !map.is_empty())
        || !matches!(capability, ReasoningCapability::Unsupported);
    ProviderModelInfo {
        origin: config.origin,
        name: config.name.clone(),
        family: config.family.clone(),
        release_date: config.release_date.clone(),
        status: config.status.clone(),
        capabilities: config.capabilities.clone(),
        wire_api: Some(config.wire_api.unwrap_or(provider_wire_api)),
        context_window: config.context_window,
        effective_context_window_percent: config.effective_context_window_percent,
        max_tokens: config.max_tokens,
        temperature: config.temperature,
        top_p: config.top_p,
        top_k: config.top_k,
        reasoning_capability: config.reasoning_capability.clone(),
        reasoning: if project_thinking {
            Some(reasoning)
        } else {
            config.reasoning
        },
        thinking_level_map: if thinking_level_map.is_empty() {
            None
        } else {
            Some(thinking_level_map)
        },
        reasoning_implementation: config.reasoning_implementation.clone(),
        default_reasoning_effort: config.default_reasoning_effort,
        default_reasoning_selection: config.default_reasoning_selection.clone(),
        base_instructions: config.base_instructions.clone(),
        input_modalities: config.input_modalities.clone(),
        channel: config.channel.clone(),
        truncation_policy: config
            .truncation_policy
            .and_then(|policy| serde_json::to_value(policy).ok()),
        supports_image_detail_original: config.supports_image_detail_original,
        web_search: config
            .web_search
            .as_ref()
            .and_then(|value| serde_json::to_value(value).ok()),
        web_fetch: config
            .web_fetch
            .as_ref()
            .and_then(|value| serde_json::to_value(value).ok()),
        cost: config.cost.clone(),
        metadata: config.metadata.clone(),
        request: config.request.clone(),
        options: config.options.clone(),
        headers: config.headers.clone(),
        variants: config
            .variants
            .iter()
            .map(|(variant_id, variant)| {
                (
                    variant_id.clone(),
                    ProviderModelVariant {
                        label: variant.label.clone(),
                        disabled: variant.disabled,
                        request_model: variant.request_model.clone(),
                        request: variant.request.clone(),
                        options: variant.options.clone(),
                        headers: variant.headers.clone(),
                    },
                )
            })
            .collect(),
        default_variant: config.default_variant.clone(),
        enabled: config.enabled,
        priority: config.priority,
    }
}

/// Errors produced while loading the builtin catalog.
#[derive(Debug, thiserror::Error)]
pub enum PresetModelCatalogError {
    /// Parsing the bundled provider directory failed.
    #[error("failed to parse builtin provider catalog: {0}")]
    Parse(#[from] serde_json::Error),
    /// Writing or validating user provider catalog config failed.
    #[error("invalid provider catalog config: {message}")]
    InvalidProviderConfig { message: String },
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use pretty_assertions::assert_eq;

    use super::{PresetModelCatalog, default_base_instructions};
    use crate::{
        Model, ModelCatalog, ProviderInfo, ProviderWireApi, ReasoningCapability, ThinkingLevelMap,
    };
    use devo_protocol::resolve_thinking_fields_for_model_info;

    #[test]
    fn builtin_models_load_from_provider_directory() {
        let catalog = PresetModelCatalog::load().expect("load provider catalog");
        assert!(!catalog.list_visible().is_empty());
        assert_eq!(catalog.list_visible()[0].slug, "openai/gpt-5.5");
    }

    #[test]
    fn builtin_catalog_resolves_visible_defaults() {
        let catalog = PresetModelCatalog::load().expect("load catalog");
        let model = catalog.resolve_for_turn(None).expect("resolve default");
        assert!(!model.slug.is_empty());
    }

    #[test]
    fn default_base_instructions_are_available() {
        assert!(!default_base_instructions().trim().is_empty());
    }

    #[test]
    fn builtin_models_have_channel_fields() {
        let catalog = PresetModelCatalog::load().expect("load provider catalog");
        assert!(
            catalog
                .list_visible()
                .iter()
                .any(|model| model.channel.as_deref() == Some("DeepSeek"))
        );
    }

    #[test]
    fn provider_catalog_uses_provider_model_references_and_accepts_custom_models() {
        let config = crate::ProviderConfigFile {
            providers: BTreeMap::from([(
                "local".to_string(),
                crate::ProviderConfigEntry {
                    wire_api: Some(ProviderWireApi::OpenAIResponses),
                    models: BTreeMap::from([(
                        "qwen3".to_string(),
                        crate::ProviderModelConfig {
                            name: Some("Qwen 3".to_string()),
                            context_window: Some(131_072),
                            ..crate::ProviderModelConfig::default()
                        },
                    )]),
                    ..crate::ProviderConfigEntry::default()
                },
            )]),
            model: Some("local/qwen3".to_string()),
            ..crate::ProviderConfigFile::default()
        };

        let catalog =
            PresetModelCatalog::load_from_provider_config(&config).expect("load provider catalog");
        assert_eq!(
            catalog
                .resolve_for_turn(None)
                .expect("resolve default")
                .slug,
            "local/qwen3"
        );
        assert_eq!(
            catalog
                .get("local/qwen3")
                .expect("custom model")
                .display_name,
            "Qwen 3"
        );
        assert_eq!(
            catalog.get("local/qwen3").expect("custom model").provider,
            ProviderWireApi::OpenAIResponses
        );
    }

    /// Trace: L2-DES-MODEL-002, L2-DES-MODEL-003
    /// Verifies: authored thinking fields survive catalog load and model/list metadata projection.
    #[test]
    fn custom_catalog_preserves_authored_thinking_fields() {
        let thinking_level_map = ThinkingLevelMap::from([
            ("off".to_string(), Some("none".to_string())),
            ("high".to_string(), Some("maximum".to_string())),
        ]);
        let config = crate::ProviderConfigFile {
            providers: BTreeMap::from([(
                "custom".to_string(),
                crate::ProviderConfigEntry {
                    models: BTreeMap::from([(
                        "reasoner".to_string(),
                        crate::ProviderModelConfig {
                            reasoning_capability: Some(ReasoningCapability::Toggle),
                            reasoning: Some(true),
                            thinking_level_map: Some(thinking_level_map.clone()),
                            ..crate::ProviderModelConfig::default()
                        },
                    )]),
                    ..crate::ProviderConfigEntry::default()
                },
            )]),
            ..crate::ProviderConfigFile::default()
        };

        let catalog =
            PresetModelCatalog::load_from_provider_config(&config).expect("load custom catalog");
        assert_eq!(
            catalog
                .get("custom/reasoner")
                .and_then(|model| model.thinking_level_map.clone()),
            Some(thinking_level_map.clone())
        );
        let info = &catalog.list_provider_models("custom")["reasoner"];
        assert_eq!(info.reasoning, Some(true));
        assert_eq!(info.thinking_level_map, Some(thinking_level_map));
    }

    /// Trace: L2-DES-MODEL-002
    /// Verifies: sparse user overlays still resolve pure builtin model defaults.
    #[test]
    fn effective_provider_catalog_resolves_builtin_model_under_sparse_overlay() {
        let overlay = crate::ProviderConfigFile {
            model: Some("deepseek/deepseek-v4-flash".to_string()),
            providers: BTreeMap::from([(
                "deepseek".to_string(),
                crate::ProviderConfigEntry {
                    base_url: Some("https://api.deepseek.com/anthropic".to_string()),
                    wire_api: Some(ProviderWireApi::AnthropicMessages),
                    credential: Some("deepseek".to_string()),
                    enabled: Some(true),
                    models: BTreeMap::from([(
                        "deepseek-flash".to_string(),
                        crate::ProviderModelConfig {
                            name: Some("DeepSeek Flash".to_string()),
                            ..crate::ProviderModelConfig::default()
                        },
                    )]),
                    ..crate::ProviderConfigEntry::default()
                },
            )]),
            ..crate::ProviderConfigFile::default()
        };

        let effective =
            super::effective_provider_catalog(&overlay).expect("merge builtin + overlay");
        let selection = effective
            .resolve_model(Some("deepseek/deepseek-v4-flash"))
            .expect("resolve builtin default through sparse overlay");
        assert_eq!(selection.provider_id, "deepseek");
        assert_eq!(selection.model_id, "deepseek-v4-flash");
        assert_eq!(selection.wire_api, ProviderWireApi::AnthropicMessages);
        assert!(
            effective.providers["deepseek"]
                .models
                .contains_key("deepseek-flash"),
            "custom models from the overlay must remain"
        );
    }

    /// Trace: L2-DES-MODEL-003
    /// Verifies: DeepSeek builtin `reasoning_capability.levels` drive chips and
    /// the authored map stays verbatim upstream (pi-ai import): the API's real
    /// `max` level is aliased at the UI's `xhigh` slot (`xhigh` → wire `max`),
    /// so selecting xhigh still sends the model's strongest level.
    #[test]
    fn deepseek_v4_flash_thinking_levels_follow_builtin_capability() {
        let catalog =
            PresetModelCatalog::load_from_provider_config(&crate::ProviderConfigFile::default())
                .expect("load builtin provider catalog");
        let model = catalog
            .get("deepseek/deepseek-v4-flash")
            .expect("deepseek-v4-flash");
        let (reasoning, map, available) = resolve_thinking_fields_for_model_info(
            &model.reasoning_capability,
            model.reasoning,
            model.thinking_level_map.as_ref(),
        );
        assert!(reasoning);
        assert_eq!(map.get("high"), Some(&Some("high".to_string())));
        assert_eq!(map.get("xhigh"), Some(&Some("max".to_string())));
        assert_eq!(map.get("max"), Some(&None));
        assert_eq!(
            available,
            vec!["off".to_string(), "high".to_string(), "xhigh".to_string()]
        );
        // The selection layer keeps the canonical level; the wire layer maps
        // it through `thinking_level_map` ("xhigh" → API level "max").
        assert_eq!(
            model
                .normalize_reasoning_effort_selection(Some("xhigh"))
                .as_deref(),
            Some("xhigh")
        );
        assert_eq!(
            model
                .normalize_reasoning_effort_selection(Some("disabled"))
                .as_deref(),
            Some("off")
        );
    }

    /// Trace: L2-DES-AUTH-001, L2-DES-MODEL-002
    /// Verifies: builtin catalog mirrors pi-ai static providers plus Devo-local templates.
    #[test]
    fn builtin_provider_catalog_contains_current_cloud_and_local_models() {
        let catalog =
            PresetModelCatalog::load_from_provider_config(&crate::ProviderConfigFile::default())
                .expect("load builtin provider catalog");

        assert_eq!(
            catalog
                .resolve_for_turn(None)
                .expect("resolve builtin default")
                .slug,
            "openai/gpt-5.5"
        );
        // pi-ai static catalog (supported wire APIs only)
        for model in [
            "openai/gpt-5.5",
            "anthropic/claude-sonnet-4-6",
            "openai-codex/gpt-5.4",
            "github-copilot/gpt-5.4",
            "xai/grok-4.6",
            "google/gemini-2.5-pro",
            "deepseek/deepseek-v4-flash",
            "zai/glm-5.3-flash",
        ] {
            assert!(
                catalog.get(model).is_some(),
                "missing pi-ai-derived builtin model {model}"
            );
        }
        // Devo-local templates preserved alongside the pi-ai import
        for model in [
            "kimi/kimi-k3",
            "zhipu/glm-5.3-flash",
            "qwen/qwen3.7-plus",
            "tencent/hunyuan-a13b",
            "poolside/laguna-s-2.1",
        ] {
            assert!(
                catalog.get(model).is_some(),
                "missing Devo-local builtin model {model}"
            );
        }
        assert!(
            catalog
                .list_providers()
                .iter()
                .any(|provider| provider.id == "ollama"),
            "missing builtin ollama provider"
        );
        for oauth_provider in ["openai-codex", "anthropic", "github-copilot", "xai"] {
            assert!(
                catalog
                    .list_providers()
                    .iter()
                    .any(|provider| provider.id == oauth_provider),
                "missing builtin OAuth provider {oauth_provider}"
            );
            assert!(
                !catalog.list_provider_models(oauth_provider).is_empty(),
                "oauth provider {oauth_provider} must ship at least one model"
            );
        }
        assert!(
            catalog
                .list_providers()
                .iter()
                .any(|provider| provider.id == "openai"),
            "missing builtin OpenAI API provider"
        );
        assert!(
            catalog.list_provider_models("ollama").is_empty(),
            "ollama template must not ship placeholder models"
        );

        let zai_models = catalog
            .list_visible()
            .into_iter()
            .filter(|model| model.slug.starts_with("zai/"))
            .map(|model| model.slug.clone())
            .collect::<Vec<_>>();
        assert!(
            zai_models.iter().any(|slug| slug == "zai/glm-5.3-flash"),
            "expected zai/glm-5.3-flash in {zai_models:?}"
        );

        let zhipu_models = catalog
            .list_visible()
            .into_iter()
            .filter(|model| model.slug.starts_with("zhipu/"))
            .map(|model| model.slug.clone())
            .collect::<Vec<_>>();
        assert_eq!(zhipu_models, ["zhipu/glm-5.3", "zhipu/glm-5.3-flash"]);

        assert_eq!(
            catalog
                .get("deepseek/deepseek-v4-flash")
                .expect("deepseek model")
                .provider,
            ProviderWireApi::OpenAIChatCompletions
        );

        let providers = catalog.list_providers();
        assert_eq!(
            providers
                .iter()
                .find(|provider| provider.id == "zhipu")
                .cloned(),
            Some(ProviderInfo {
                id: "zhipu".to_string(),
                name: "Zhipu AI".to_string(),
                description: Some("China BigModel GLM API".to_string()),
                base_url: Some("https://open.bigmodel.cn/api/paas/v4".to_string()),
                credential: None,
                headers: BTreeMap::new(),
                options: None,
                request: None,
                compat: None,
                wire_apis: vec![ProviderWireApi::OpenAIChatCompletions],
                model_overrides: BTreeMap::new(),
                models: BTreeMap::new(),
                enabled: true,
            })
        );
        assert_eq!(
            providers
                .iter()
                .find(|provider| provider.id == "deepseek")
                .map(|provider| provider.wire_apis.clone()),
            Some(vec![ProviderWireApi::OpenAIChatCompletions])
        );
    }

    #[test]
    fn provider_catalog_materializes_a_minimal_referenced_custom_model() {
        let config = crate::ProviderConfigFile {
            model: Some("local/qwen3".to_string()),
            providers: BTreeMap::from([(
                "local".to_string(),
                crate::ProviderConfigEntry::default(),
            )]),
            ..crate::ProviderConfigFile::default()
        };

        let catalog =
            PresetModelCatalog::load_from_provider_config(&config).expect("load provider catalog");

        assert_eq!(
            catalog.get("local/qwen3").expect("referenced custom model"),
            &Model {
                slug: "local/qwen3".to_string(),
                display_name: "qwen3".to_string(),
                default_reasoning_effort: None,
                base_instructions: default_base_instructions().to_string(),
                ..Model::default()
            }
        );
    }
}

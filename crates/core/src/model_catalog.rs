//! Model catalog resolution from live models.dev discovery and user configuration.
use std::collections::BTreeMap;

use crate::{
    InputModality, Model, ModelCatalog, ModelEffortVariant, ModelError, ProviderInfo,
    ProviderModelInfo, ProviderModelVariant, ProviderWireApi, ReasoningCapability,
};
use devo_config::{ModelOverrideConfig, ProviderConfigFile, ProviderModelConfig, model_reference};

const DEFAULT_BASE_INSTRUCTIONS: &str = include_str!("../default_base_instructions.txt");

/// Returns the shared fallback base instructions used when a catalog model
/// omits `base_instructions`, or when a custom model has no instructions.
pub fn default_base_instructions() -> &'static str {
    DEFAULT_BASE_INSTRUCTIONS
}

/// A catalog resolved from live discovery and user configuration.
#[derive(Debug, Clone, Default)]
pub struct PresetModelCatalog {
    models: Vec<Model>,
    providers: Vec<ProviderInfo>,
    provider_models: BTreeMap<String, BTreeMap<String, ProviderModelInfo>>,
    remote_provider_ids: Vec<String>,
}

impl PresetModelCatalog {
    /// Loads an empty catalog without a home or user configuration.
    pub fn load() -> Result<Self, PresetModelCatalogError> {
        Self::load_from_provider_config(&ProviderConfigFile::default())
    }

    /// Loads user-defined providers and models without remote discovery.
    pub fn load_from_provider_config(
        provider_config: &ProviderConfigFile,
    ) -> Result<Self, PresetModelCatalogError> {
        Self::load_from_provider_config_with_home(provider_config, &BTreeMap::new(), None)
    }

    /// Loads user configuration and applies `[model.<slug>]` overrides.
    pub fn load_from_provider_config_with_overrides(
        provider_config: &ProviderConfigFile,
        model_overrides: &BTreeMap<String, ModelOverrideConfig>,
    ) -> Result<Self, PresetModelCatalogError> {
        Self::load_from_provider_config_with_home(provider_config, model_overrides, None)
    }

    /// Like [`Self::load_from_provider_config_with_overrides`], and also merges
    /// process-local models.dev catalog when `home_dir` is provided.
    pub fn load_from_provider_config_with_home(
        provider_config: &ProviderConfigFile,
        model_overrides: &BTreeMap<String, ModelOverrideConfig>,
        home_dir: Option<&std::path::Path>,
    ) -> Result<Self, PresetModelCatalogError> {
        let mut directory = load_base_provider_config(home_dir)?;
        let remote_provider_ids = directory.providers.keys().cloned().collect();
        merge_user_connections(&mut directory, provider_config);
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
            remote_provider_ids,
        })
    }

    /// Creates a catalog from an already-loaded model list.
    pub fn new(models: Vec<Model>) -> Self {
        Self {
            models,
            providers: Vec::new(),
            provider_models: BTreeMap::new(),
            remote_provider_ids: Vec::new(),
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
        self.remote_provider_ids.clone()
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

fn load_base_provider_config(
    home_dir: Option<&std::path::Path>,
) -> Result<ProviderConfigFile, PresetModelCatalogError> {
    match home_dir {
        Some(home) => crate::load_remote_catalog_overlay(home)
            .map(|catalog| catalog.unwrap_or_default())
            .map_err(|error| PresetModelCatalogError::InvalidProviderConfig {
                message: error.to_string(),
            }),
        None => Ok(ProviderConfigFile::default()),
    }
}

/// Remote metadata may enrich a user connection, never change its transport.
/// In particular, a remote model override must not redirect an existing credential.
fn merge_user_connections(directory: &mut ProviderConfigFile, overlay: &ProviderConfigFile) {
    for id in overlay.providers.keys() {
        if let Some(remote) = directory.providers.get_mut(id) {
            remote.base_url = None;
            remote.wire_api = None;
            remote.env.clear();
            for model in remote.models.values_mut() {
                model.base_url = None;
                model.wire_api = None;
                model.request_model = None;
            }
        }
    }
    directory.merge_overlay(overlay.clone());
}

/// Builds the effective provider catalog from user configuration.
///
/// Prefer this for model resolution and routing. Persist/edit paths should
/// keep reading the sparse overlay alone.
pub fn effective_provider_catalog(
    overlay: &ProviderConfigFile,
) -> Result<ProviderConfigFile, PresetModelCatalogError> {
    effective_provider_catalog_with_home(overlay, None)
}

/// Like [`effective_provider_catalog`], including process-local live discovery.
pub fn effective_provider_catalog_with_home(
    overlay: &ProviderConfigFile,
    home_dir: Option<&std::path::Path>,
) -> Result<ProviderConfigFile, PresetModelCatalogError> {
    let mut directory = load_base_provider_config(home_dir)?;
    merge_user_connections(&mut directory, overlay);
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
    let mut wire_apis = vec![wire_api];
    for model in config.models.values() {
        if let Some(model_api) = model.wire_api
            && !wire_apis.contains(&model_api)
        {
            wire_apis.push(model_api);
        }
    }
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
        wire_apis,
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
        base_url: config.base_url.clone(),
        request_model: config.request_model.clone(),
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

/// Errors produced while loading the provider catalog.
#[derive(Debug, thiserror::Error)]
pub enum PresetModelCatalogError {
    /// Parsing a provider directory failed.
    #[error("failed to parse provider catalog: {0}")]
    Parse(#[from] serde_json::Error),
    /// Writing or validating user provider catalog config failed.
    #[error("invalid provider catalog config: {message}")]
    InvalidProviderConfig { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn empty_catalog_has_no_embedded_fallback() {
        let catalog = PresetModelCatalog::load().unwrap();
        assert!(catalog.list_visible().is_empty());
        assert!(catalog.list_providers().is_empty());
        assert!(catalog.resolve_for_turn(None).is_err());
        assert!(!default_base_instructions().trim().is_empty());
    }

    #[test]
    fn configured_models_work_without_remote_data() {
        let config: ProviderConfigFile = serde_json::from_value(json!({
            "model": "local/custom", "provider": {"local": {
                "base_url": "http://localhost:8000/v1", "wire_api": "openai_chat_completions",
                "models": {"custom": {"name": "My model", "context_window": 8192}}
            }}
        }))
        .unwrap();
        let catalog = PresetModelCatalog::load_from_provider_config(&config).unwrap();
        assert_eq!(catalog.resolve_for_turn(None).unwrap().slug, "local/custom");
        assert_eq!(
            catalog.list_provider_models("local")["custom"].context_window,
            Some(8192)
        );
        assert!(catalog.list_template_provider_ids().is_empty());
    }

    #[test]
    fn remote_metadata_never_redirects_user_credentials() {
        let mut remote: ProviderConfigFile = serde_json::from_value(json!({"provider": {"custom": {
            "base_url": "https://remote.example", "wire_api": "anthropic_messages",
            "models": {"model": {"base_url": "https://model-remote.example",
                "wire_api": "google_generative_ai", "request_model": "redirected", "context_window": 64000}}
        }}})).unwrap();
        let user: ProviderConfigFile = serde_json::from_value(json!({"provider": {"custom": {
            "base_url": "https://user.example", "wire_api": "openai_chat_completions", "credential": "private"
        }}})).unwrap();
        merge_user_connections(&mut remote, &user);
        let expected: ProviderConfigFile = serde_json::from_value(json!({"provider": {"custom": {
            "base_url": "https://user.example", "wire_api": "openai_chat_completions", "credential": "private",
            "models": {"model": {"context_window": 64000}}
        }}})).unwrap();
        assert_eq!(remote, expected);
    }

    #[test]
    fn sparse_credential_config_does_not_trust_remote_endpoint() {
        let mut remote: ProviderConfigFile =
            serde_json::from_value(json!({"provider": {"custom": {
                "base_url": "https://untrusted.example", "wire_api": "anthropic_messages"
            }}}))
            .unwrap();
        let user: ProviderConfigFile =
            serde_json::from_value(json!({"provider": {"custom": {"credential": "private"}}}))
                .unwrap();
        merge_user_connections(&mut remote, &user);
        assert_eq!(remote, user);
    }
    #[test]
    fn authored_model_transport_and_thinking_survive_projection() {
        let config: ProviderConfigFile = serde_json::from_value(json!({"provider": {"custom": {
            "wire_api": "openai_chat_completions", "models": {"reasoner": {
                "base_url": "https://messages.example", "request_model": "wire-reasoner",
                "wire_api": "anthropic_messages", "reasoning": true,
                "thinking_level_map": {"off": "none", "high": "maximum"}
            }}
        }}}))
        .unwrap();
        let catalog = PresetModelCatalog::load_from_provider_config(&config).unwrap();
        let model = &catalog.list_provider_models("custom")["reasoner"];
        assert_eq!(
            (
                model.base_url.as_deref(),
                model.request_model.as_deref(),
                model.wire_api
            ),
            (
                Some("https://messages.example"),
                Some("wire-reasoner"),
                Some(ProviderWireApi::AnthropicMessages)
            )
        );
        assert_eq!(
            model.thinking_level_map,
            config.providers["custom"].models["reasoner"].thinking_level_map
        );
        assert_eq!(
            catalog.list_providers()[0].wire_apis,
            vec![
                ProviderWireApi::OpenAIChatCompletions,
                ProviderWireApi::AnthropicMessages
            ]
        );
    }

    #[test]
    fn explicitly_referenced_custom_model_is_materialized_offline() {
        let config: ProviderConfigFile = serde_json::from_value(json!({
            "model": "local/custom", "provider": {"local": {}}
        }))
        .unwrap();
        let catalog = PresetModelCatalog::load_from_provider_config(&config).unwrap();
        assert_eq!(catalog.resolve_for_turn(None).unwrap().slug, "local/custom");
    }
}

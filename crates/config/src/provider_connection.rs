//! Persistence for the canonical provider Connection contract.

use std::collections::BTreeMap;

use devo_protocol::{ProviderInfo, ProviderModelInfo, ProviderModelOrigin, ProviderModelVariant};

use super::{AppConfigLoader, AppConfigStore};
use crate::{
    ProviderConfigEntry, ProviderConfigFile, ProviderModelConfig, ProviderModelVariantConfig,
    default_provider_credential_id, migrate_legacy_provider_config_file, non_empty_string,
    read_provider_catalog_config, upsert_user_auth_api_key, write_provider_catalog_config,
};

impl AppConfigStore {
    /// Returns the effective user provider Connections in canonical form.
    ///
    /// Built-in directory templates are supplied by `ModelCatalog`; this
    /// method only exposes persisted user entries and their explicit models.
    pub fn provider_connections(&self) -> anyhow::Result<Vec<ProviderInfo>> {
        Ok(self
            .load_editable_provider_catalog()?
            .providers
            .iter()
            .map(|(provider_id, provider)| provider_info_from_config(provider_id, provider))
            .collect())
    }

    /// Returns the model directory explicitly stored for each user Connection.
    ///
    /// The embedded provider directory is deliberately excluded. This keeps
    /// Connection management separate from read-only provider templates.
    pub fn provider_connection_models(
        &self,
    ) -> anyhow::Result<BTreeMap<String, BTreeMap<String, ProviderModelInfo>>> {
        let config = self.load_editable_provider_catalog()?;

        Ok(config
            .providers
            .into_iter()
            .map(|(provider_id, provider)| {
                let models = provider
                    .models
                    .iter()
                    .map(|(model_id, model)| {
                        (model_id.clone(), provider_model_info_from_config(model))
                    })
                    .collect();
                (provider_id, models)
            })
            .collect())
    }

    /// Removes one model from a user-created provider Connection.
    pub fn remove_provider_model(
        &mut self,
        provider_id: &str,
        model_id: &str,
    ) -> anyhow::Result<()> {
        let provider_id = non_empty_string(provider_id)
            .ok_or_else(|| anyhow::anyhow!("provider id must not be empty"))?;
        let model_id = non_empty_string(model_id)
            .ok_or_else(|| anyhow::anyhow!("model id must not be empty"))?;
        let model_id = model_id
            .strip_prefix(&format!("{provider_id}/"))
            .unwrap_or(&model_id)
            .to_string();
        if !self
            .provider_connection_ids()?
            .iter()
            .any(|connected_id| connected_id == &provider_id)
        {
            anyhow::bail!("provider {provider_id} is not a user Connection");
        }

        let editable = self.load_editable_provider_catalog()?;
        let model = editable
            .providers
            .get(&provider_id)
            .and_then(|provider| provider.models.get(&model_id));
        if model.and_then(|model| model.origin) != Some(ProviderModelOrigin::User) {
            anyhow::bail!(
                "catalog model {provider_id}/{model_id} is read-only; only manually added models can be removed"
            );
        }

        let connection_path = self.user_provider_config_file();
        let custom_path = self.user_custom_provider_config_file();
        let mut connection = read_provider_catalog_config(&connection_path)?;
        let mut custom = read_provider_catalog_config(&custom_path)?;
        if let Some(provider) = connection.providers.get_mut(&provider_id) {
            provider.models.remove(&model_id);
            provider.model_overrides.remove(&model_id);
        } else if let Some(provider) = custom.providers.get_mut(&provider_id) {
            provider.models.remove(&model_id);
            provider.model_overrides.remove(&model_id);
        }
        let model_prefix = format!("{provider_id}/{model_id}");
        for config in [&mut connection, &mut custom] {
            if config.model.as_deref().is_some_and(|model| {
                model == model_prefix || model.starts_with(&format!("{model_prefix}/"))
            }) {
                config.model = None;
            }
            if config.small_model.as_deref().is_some_and(|model| {
                model == model_prefix || model.starts_with(&format!("{model_prefix}/"))
            }) {
                config.small_model = None;
            }
        }

        write_provider_catalog_config(&connection_path, &connection)?;
        write_provider_catalog_config(&custom_path, &custom)?;
        super::clear_user_session_default_if_matches(
            &self.user_config_file,
            "model",
            &model_prefix,
        )?;
        super::clear_user_session_default_if_matches(
            &self.user_config_file,
            "small_model",
            &model_prefix,
        )?;
        migrate_legacy_provider_config_file(&self.user_config_file)?;
        self.config = self
            .loader
            .load(self.workspace_root.as_deref())
            .map_err(|error| anyhow::anyhow!(error))?;
        Ok(())
    }

    /// Persists one canonical provider Connection, including its complete
    /// nested model directory, without exposing the legacy binding shape.
    pub fn upsert_provider_connection(
        &mut self,
        provider: ProviderInfo,
        default_model: Option<String>,
        small_model: Option<String>,
        api_key: Option<String>,
    ) -> anyhow::Result<ProviderInfo> {
        self.upsert_provider_connection_with_baseline(
            provider,
            default_model,
            small_model,
            api_key,
            None,
        )
    }

    /// Like [`Self::upsert_provider_connection`], but when `builtin_baseline` is
    /// the embedded catalog entry for this provider id, only novel models and
    /// sparse `model_overrides` are written (sparse overlay semantics).
    ///
    /// Builtin Connections are stored in `providers.json`. User-owned custom
    /// providers/models are stored in `custom-providers.json`.
    pub fn upsert_provider_connection_with_baseline(
        &mut self,
        provider: ProviderInfo,
        default_model: Option<String>,
        small_model: Option<String>,
        api_key: Option<String>,
        builtin_baseline: Option<&ProviderConfigEntry>,
    ) -> anyhow::Result<ProviderInfo> {
        let provider_id = non_empty_string(&provider.id)
            .or_else(|| non_empty_string(&provider.name))
            .ok_or_else(|| anyhow::anyhow!("provider id or name must not be empty"))?;
        if provider.wire_apis.is_empty() {
            anyhow::bail!("wire_apis must contain at least one wire API");
        }
        for (model_id, model) in &provider.models {
            validate_model(&provider_id, &provider.wire_apis, model_id, model)?;
        }

        let target_config_file = if builtin_baseline.is_some() {
            self.user_provider_config_file()
        } else {
            self.user_custom_provider_config_file()
        };
        let other_config_file = if builtin_baseline.is_some() {
            self.user_custom_provider_config_file()
        } else {
            self.user_provider_config_file()
        };
        let mut config = read_provider_catalog_config(&target_config_file)?;
        let mut other = read_provider_catalog_config(&other_config_file)?;
        if other.providers.remove(&provider_id).is_some() {
            write_provider_catalog_config(&other_config_file, &other)?;
        }

        let api_key = api_key.as_deref().and_then(non_empty_string);
        let credential = provider
            .credential
            .as_deref()
            .and_then(non_empty_string)
            .map(|id| crate::provider_id_from_credential_id(&id))
            .or_else(|| {
                config
                    .providers
                    .get(&provider_id)
                    .and_then(|entry| entry.credential.clone())
                    .map(|id| crate::provider_id_from_credential_id(&id))
            })
            .or_else(|| {
                other
                    .providers
                    .get(&provider_id)
                    .and_then(|entry| entry.credential.clone())
                    .map(|id| crate::provider_id_from_credential_id(&id))
            })
            .or_else(|| {
                api_key
                    .as_ref()
                    .map(|_| default_provider_credential_id(&provider_id))
            });
        if let (Some(api_key), Some(credential_id)) = (api_key, credential.as_deref()) {
            upsert_user_auth_api_key(self.user_config_dir(), credential_id, &api_key)
                .map_err(|error| anyhow::anyhow!(error))?;
        }

        {
            let entry = config.providers.entry(provider_id.clone()).or_default();
            apply_provider_info(entry, &provider, credential.clone());
            for (model_id, mut model_info) in provider.models {
                let existing = entry
                    .models
                    .get(&model_id)
                    .or_else(|| entry.model_overrides.get(&model_id));
                model_info.origin = if existing.and_then(|model| model.origin)
                    == Some(ProviderModelOrigin::Remote)
                    || model_info.origin == Some(ProviderModelOrigin::Remote)
                {
                    Some(ProviderModelOrigin::Remote)
                } else if builtin_baseline
                    .is_some_and(|baseline| baseline.models.contains_key(&model_id))
                {
                    None
                } else if let Some(existing) = existing {
                    existing.origin
                } else {
                    Some(ProviderModelOrigin::User)
                };
                entry
                    .models
                    .insert(model_id, provider_model_config_from_info(model_info));
            }
            if let Some(model) = default_model.as_deref().and_then(non_empty_string) {
                if !provider_has_model(entry, &provider_id, &model)
                    && !provider_has_model_in_overrides_or_baseline(
                        entry,
                        builtin_baseline,
                        &provider_id,
                        &model,
                    )
                {
                    anyhow::bail!("default model `{model}` is not in provider `{provider_id}`");
                }
                // Persist session default in config.toml, not providers.json.
                super::write_user_session_default(&self.user_config_file, "model", &model)?;
            }
            if let Some(model) = small_model.as_deref().and_then(non_empty_string) {
                if !provider_has_model(entry, &provider_id, &model)
                    && !provider_has_model_in_overrides_or_baseline(
                        entry,
                        builtin_baseline,
                        &provider_id,
                        &model,
                    )
                {
                    anyhow::bail!("small model `{model}` is not in provider `{provider_id}`");
                }
                super::write_user_session_default(&self.user_config_file, "small_model", &model)?;
            }

            // Builtin providers persist sparse overlays; custom providers stay full.
            if let Some(baseline) = builtin_baseline {
                sparsify_provider_entry_against_builtin(entry, baseline);
            }
        }

        // Session defaults are preferences in config.toml, not catalog fields.
        config.model = None;
        config.small_model = None;
        config.reasoning_effort = None;

        write_provider_catalog_config(&target_config_file, &config)?;
        migrate_legacy_provider_config_file(&self.user_config_file)?;
        self.config = self
            .loader
            .load(self.workspace_root.as_deref())
            .map_err(|error| anyhow::anyhow!(error))?;

        let effective = self
            .config
            .provider_catalog
            .providers
            .get(&provider_id)
            .ok_or_else(|| anyhow::anyhow!("provider `{provider_id}` was not persisted"))?;
        Ok(provider_info_from_config(&provider_id, effective))
    }

    fn load_editable_provider_catalog(&self) -> anyhow::Result<ProviderConfigFile> {
        let mut config = read_provider_catalog_config(&self.user_provider_config_file())?;
        config.merge_overlay(read_provider_catalog_config(
            &self.user_custom_provider_config_file(),
        )?);
        Ok(config)
    }
}

/// Rewrites a user provider entry so built-in models become sparse
/// `model_overrides` (or are omitted when identical) and only novel models stay
/// in `models` — matching the `models.json` sparse overlay rules.
pub fn sparsify_provider_entry_against_builtin(
    entry: &mut ProviderConfigEntry,
    builtin: &ProviderConfigEntry,
) {
    let mut custom_models = BTreeMap::new();
    let mut overrides = entry.model_overrides.clone();
    for (model_id, model) in std::mem::take(&mut entry.models) {
        match builtin.models.get(&model_id) {
            None => {
                custom_models.insert(model_id, model);
            }
            Some(baseline_model) if model == *baseline_model => {
                // Identical to builtin — inherit; do not persist.
            }
            Some(_) => {
                overrides.insert(model_id, model);
            }
        }
    }
    entry.models = custom_models;
    entry.model_overrides = overrides;

    if entry.name.is_some() && entry.name == builtin.name {
        // Keep display name; harmless. Prefer omitting when equal to shrink file.
        entry.name = None;
    }
    if entry.description.is_some() && entry.description == builtin.description {
        entry.description = None;
    }
}

fn provider_has_model_in_overrides_or_baseline(
    provider: &ProviderConfigEntry,
    builtin_baseline: Option<&ProviderConfigEntry>,
    provider_id: &str,
    model: &str,
) -> bool {
    let model_id = model
        .strip_prefix(&format!("{provider_id}/"))
        .unwrap_or(model);
    if provider.model_overrides.contains_key(model_id) {
        return true;
    }
    builtin_baseline.is_some_and(|baseline| baseline.models.contains_key(model_id))
}

fn provider_has_model(provider: &ProviderConfigEntry, provider_id: &str, model: &str) -> bool {
    let model_id = model
        .strip_prefix(&format!("{provider_id}/"))
        .unwrap_or(model);
    if provider.models.contains_key(model_id) {
        return true;
    }
    model_id
        .rsplit_once('/')
        .filter(|(base_model_id, variant_id)| {
            provider
                .models
                .get(*base_model_id)
                .is_some_and(|model| model.variants.contains_key(*variant_id))
        })
        .is_some()
}

fn validate_model(
    provider_id: &str,
    wire_apis: &[devo_protocol::ProviderWireApi],
    model_id: &str,
    model: &ProviderModelInfo,
) -> anyhow::Result<()> {
    if model_id.trim().is_empty() {
        anyhow::bail!("model id cannot be empty");
    }
    if let Some(wire_api) = model.wire_api
        && !wire_apis.contains(&wire_api)
    {
        anyhow::bail!("model `{provider_id}/{model_id}` uses an unsupported wire API");
    }
    if let Some(default_variant) = model.default_variant.as_deref()
        && !model.variants.contains_key(default_variant)
    {
        anyhow::bail!(
            "model `{provider_id}/{model_id}` refers to missing default variant `{default_variant}`"
        );
    }
    Ok(())
}

fn apply_provider_info(
    entry: &mut ProviderConfigEntry,
    provider: &ProviderInfo,
    credential: Option<String>,
) {
    entry.name = non_empty_string(&provider.name);
    entry.description = provider.description.as_deref().and_then(non_empty_string);
    entry.base_url = provider.base_url.as_deref().and_then(non_empty_string);
    entry.credential = credential;
    entry.headers = (!provider.headers.is_empty()).then(|| provider.headers.clone());
    entry.options = provider.options.clone();
    entry.request = provider.request.clone();
    entry.compat = provider.compat.clone();
    entry.model_overrides = provider
        .model_overrides
        .clone()
        .into_iter()
        .map(|(model_id, model)| (model_id, provider_model_config_from_info(model)))
        .collect();
    entry.wire_api = provider.wire_apis.first().copied();
    entry.enabled = Some(provider.enabled);
}

fn provider_model_config_from_info(info: ProviderModelInfo) -> ProviderModelConfig {
    ProviderModelConfig {
        origin: info.origin,
        name: info.name,
        family: info.family,
        release_date: info.release_date,
        status: info.status,
        capabilities: info.capabilities,
        wire_api: info.wire_api,
        context_window: info.context_window,
        effective_context_window_percent: info.effective_context_window_percent,
        max_tokens: info.max_tokens,
        temperature: info.temperature,
        top_p: info.top_p,
        top_k: info.top_k,
        reasoning_capability: info.reasoning_capability,
        reasoning: info.reasoning,
        thinking_level_map: info.thinking_level_map,
        reasoning_implementation: info.reasoning_implementation,
        default_reasoning_effort: info.default_reasoning_effort,
        default_reasoning_selection: info.default_reasoning_selection,
        base_instructions: info.base_instructions,
        input_modalities: info.input_modalities,
        channel: info.channel,
        truncation_policy: info
            .truncation_policy
            .and_then(|value| serde_json::from_value(value).ok()),
        supports_image_detail_original: info.supports_image_detail_original,
        web_search: info
            .web_search
            .and_then(|value| serde_json::from_value(value).ok()),
        web_fetch: info
            .web_fetch
            .and_then(|value| serde_json::from_value(value).ok()),
        cost: info.cost,
        metadata: info.metadata,
        request: info.request,
        options: info.options,
        headers: info.headers,
        variants: info
            .variants
            .into_iter()
            .map(|(variant_id, variant)| {
                (
                    variant_id,
                    ProviderModelVariantConfig {
                        label: variant.label,
                        disabled: variant.disabled,
                        request_model: variant.request_model,
                        request: variant.request,
                        options: variant.options,
                        headers: variant.headers,
                    },
                )
            })
            .collect(),
        default_variant: info.default_variant,
        enabled: info.enabled,
        priority: info.priority,
    }
}

fn provider_info_from_config(provider_id: &str, config: &ProviderConfigEntry) -> ProviderInfo {
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
        wire_apis: vec![
            config
                .wire_api
                .unwrap_or(devo_protocol::ProviderWireApi::OpenAIChatCompletions),
        ],
        model_overrides: config
            .model_overrides
            .iter()
            .map(|(model_id, model)| (model_id.clone(), provider_model_info_from_config(model)))
            .collect(),
        models: config
            .models
            .iter()
            .map(|(model_id, model)| (model_id.clone(), provider_model_info_from_config(model)))
            .collect(),
        enabled: config.enabled.unwrap_or(true),
    }
}

fn provider_model_info_from_config(config: &ProviderModelConfig) -> ProviderModelInfo {
    let capability = config
        .reasoning_capability
        .clone()
        .unwrap_or(devo_protocol::ReasoningCapability::Unsupported);
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
        || !matches!(capability, devo_protocol::ReasoningCapability::Unsupported);
    ProviderModelInfo {
        origin: config.origin,
        name: config.name.clone(),
        family: config.family.clone(),
        release_date: config.release_date.clone(),
        status: config.status.clone(),
        capabilities: config.capabilities.clone(),
        wire_api: config.wire_api,
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

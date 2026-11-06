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
    /// Live directory templates are supplied by `ModelCatalog`; this
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
    /// The remote provider directory is deliberately excluded. This keeps
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

    /// Saves a connection selected from a live directory baseline. Remote models
    /// retain only explicit transport settings; existing user metadata is preserved.
    /// The baseline is not persisted and is never required for offline requests.
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

        // Keep existing authored entries in their original file. Never delete a
        // second overlay merely because the connection was edited.
        let connection_path = self.user_provider_config_file();
        let custom_path = self.user_custom_provider_config_file();
        let connections = read_provider_catalog_config(&connection_path)?;
        let customs = read_provider_catalog_config(&custom_path)?;
        let target_is_connection = !customs.providers.contains_key(&provider_id)
            && (connections.providers.contains_key(&provider_id) || builtin_baseline.is_some());
        let (target_config_file, mut config, other) = if target_is_connection {
            (connection_path, connections, customs)
        } else {
            (custom_path, customs, connections)
        };

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
                let existing = entry.models.get(&model_id);
                let remote = existing.and_then(|model| model.origin)
                    == Some(ProviderModelOrigin::Remote)
                    || model_info.origin == Some(ProviderModelOrigin::Remote)
                    || builtin_baseline
                        .is_some_and(|baseline| baseline.models.contains_key(&model_id));
                if remote {
                    // Persist only the transport selected by the user, never a
                    // copy of the remote catalog's capabilities or pricing.
                    let model = entry.models.entry(model_id).or_default();
                    if model_info.base_url.is_some() {
                        model.base_url = model_info.base_url;
                    }
                    if model_info.request_model.is_some() {
                        model.request_model = model_info.request_model;
                    }
                    model.wire_api = model_info.wire_api.or(entry.wire_api);
                    model.origin.get_or_insert(ProviderModelOrigin::Remote);
                } else {
                    model_info.origin = existing
                        .and_then(|model| model.origin)
                        .or(model_info.origin)
                        .or(Some(ProviderModelOrigin::User));
                    entry
                        .models
                        .entry(model_id)
                        .or_default()
                        .apply_overlay(provider_model_config_from_info(model_info));
                }
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

    /// Saves the transport of a selected model without copying remote metadata.
    /// Existing user-authored model settings and credential references are preserved.
    /// A legacy connection without an explicit endpoint must be reconnected first.
    pub fn pin_provider_model(
        &mut self,
        provider_id: &str,
        model_id: &str,
        model: ProviderModelConfig,
    ) -> anyhow::Result<()> {
        let custom_path = self.user_custom_provider_config_file();
        let custom = read_provider_catalog_config(&custom_path)?;
        let (path, mut config) = if custom.providers.contains_key(provider_id) {
            (custom_path, custom)
        } else {
            let path = self.user_provider_config_file();
            let config = read_provider_catalog_config(&path)?;
            (path, config)
        };
        let entry = config
            .providers
            .get_mut(provider_id)
            .ok_or_else(|| anyhow::anyhow!("provider `{provider_id}` is not connected"))?;
        let existing = entry.models.get(model_id);
        if entry
            .base_url
            .as_deref()
            .and_then(non_empty_string)
            .is_none()
            && existing
                .and_then(|model| model.base_url.as_deref())
                .and_then(non_empty_string)
                .is_none()
        {
            anyhow::bail!(
                "provider `{provider_id}` has no pinned endpoint; reconnect it with an explicit endpoint before selecting a model"
            );
        }
        let pinned = entry.models.entry(model_id.to_string()).or_default();
        // Missing incoming transport preserves existing authored transport.
        if model.base_url.is_some() {
            pinned.base_url = model.base_url;
        }
        if model.request_model.is_some() {
            pinned.request_model = model.request_model;
        }
        pinned.wire_api = model.wire_api.or(pinned.wire_api).or(entry.wire_api);
        pinned.origin = pinned.origin.or(model.origin);
        write_provider_catalog_config(&path, &config)?;
        self.config = self
            .loader
            .load(self.workspace_root.as_deref())
            .map_err(|error| anyhow::anyhow!(error))?;
        Ok(())
    }

    fn load_editable_provider_catalog(&self) -> anyhow::Result<ProviderConfigFile> {
        let mut config = read_provider_catalog_config(&self.user_provider_config_file())?;
        config.merge_overlay(read_provider_catalog_config(
            &self.user_custom_provider_config_file(),
        )?);
        Ok(config)
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
    entry.name = non_empty_string(&provider.name).or_else(|| entry.name.clone());
    entry.description = provider.description.as_deref().and_then(non_empty_string);
    entry.base_url = provider
        .base_url
        .as_deref()
        .and_then(non_empty_string)
        .or_else(|| entry.base_url.clone());
    entry.credential = credential;
    entry.headers = (!provider.headers.is_empty()).then(|| provider.headers.clone());
    if provider.options.is_some() {
        entry.options = provider.options.clone();
    }
    if provider.request.is_some() {
        entry.request = provider.request.clone();
    }
    if provider.compat.is_some() {
        entry.compat = provider.compat.clone();
    }
    entry.model_overrides.extend(
        provider
            .model_overrides
            .clone()
            .into_iter()
            .map(|(model_id, model)| (model_id, provider_model_config_from_info(model)))
            .collect::<BTreeMap<_, _>>(),
    );
    entry.wire_api = provider.wire_apis.first().copied();
    entry.enabled = Some(provider.enabled);
}

fn provider_model_config_from_info(info: ProviderModelInfo) -> ProviderModelConfig {
    ProviderModelConfig {
        base_url: info.base_url,
        request_model: info.request_model,
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
        wire_apis: {
            let mut protocols = vec![
                config
                    .wire_api
                    .unwrap_or(devo_protocol::ProviderWireApi::OpenAIChatCompletions),
            ];
            for protocol in config.models.values().filter_map(|model| model.wire_api) {
                if !protocols.contains(&protocol) {
                    protocols.push(protocol);
                }
            }
            protocols
        },
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
        base_url: config.base_url.clone(),
        request_model: config.request_model.clone(),
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

#[cfg(test)]
#[path = "provider_connection_tests.rs"]
mod tests;

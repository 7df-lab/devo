use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;

use devo_protocol::ProviderWireApi;
use toml::Value;

use crate::ProviderConfigError;

use super::auth::read_user_auth_config;
use super::auth::upsert_user_auth_api_key;
use super::catalog::ProviderConfigEntry;
use super::catalog::ProviderConfigFile;
use super::catalog::ProviderModelConfig;
use super::persistence::non_empty_string;
use super::persistence::read_provider_catalog_config;
use super::persistence::read_provider_config_document;
use super::persistence::write_atomic;
use super::persistence::write_provider_catalog_config;
use super::schema::ConfiguredModel;
use super::schema::LegacyModelProviderConfig;
use super::schema::ModelOverrideConfig;
use super::schema::ProviderConfigSection;

/// Migrates provider-owned settings from the legacy TOML file into the
/// standalone JSON catalog during config loading.
///
/// The migration runs before the caller parses the application config. The
/// JSON file is the durable destination, while old provider tables are removed
/// only after the catalog and legacy API keys have been written successfully.
/// Existing JSON values take precedence over TOML values, so restarting cannot
/// regress a newer configuration.
pub(crate) fn migrate_legacy_provider_config_on_startup(
    legacy_config_file: &Path,
    target_config_file: &Path,
    user_config_dir: &Path,
) -> Result<(), ProviderConfigError> {
    if !legacy_config_file.exists() {
        return Ok(());
    }

    let document = read_provider_config_document(legacy_config_file)?;
    let legacy = document
        .clone()
        .try_into::<ProviderConfigSection>()
        .map_err(|error| ProviderConfigError::ParseTomlFile {
            path: legacy_config_file.to_path_buf(),
            message: error.to_string(),
        })?;
    if !has_legacy_provider_config(&legacy) {
        return Ok(());
    }

    let mut migrated = ProviderConfigFile::from_provider_config_section(&legacy);
    restore_legacy_provider_fields(&document, &mut migrated);
    merge_legacy_model_providers(&mut migrated, &legacy.model_providers);
    if migrated.model.is_none() {
        migrated.model = legacy_model_selection(&legacy, &migrated);
    }

    let consumed_model_overrides =
        migrate_legacy_model_overrides(&mut migrated, &legacy.model_overrides)?;

    migrate_legacy_api_keys(&document, &legacy, &mut migrated, user_config_dir)?;

    // Selection prefs stay in config.toml; strip them from the JSON catalog.
    let selection_model = migrated.model.take();
    let selection_effort = migrated.reasoning_effort.take();
    let selection_small = migrated.small_model.take();

    let existing = read_provider_catalog_config(target_config_file)?;
    let target_exists = target_config_file.exists();
    // Existing JSON session defaults win over legacy TOML (one-time migrate).
    let json_model = existing.model.clone();
    let json_effort = existing.reasoning_effort.clone();
    let json_small = existing.small_model.clone();
    let migrated_snapshot = migrated.clone();
    let mut merged = migrated;
    if target_exists {
        merged.merge_overlay(existing.clone());
    }
    // Never persist session defaults in providers.json.
    merged.model = None;
    merged.small_model = None;
    merged.reasoning_effort = None;
    if has_persistable_provider_config(&merged) && (!target_exists || merged != existing) {
        write_provider_catalog_config(target_config_file, &merged)?;
    }

    // Strip migrated provider tables first, then write session defaults so a
    // stale in-memory document cannot clobber the fully-qualified model.
    remove_migrated_legacy_config(
        legacy_config_file,
        document,
        &legacy,
        &migrated_snapshot,
        &consumed_model_overrides,
    )?;
    ensure_toml_session_defaults(
        legacy_config_file,
        json_model.or(selection_model).or(legacy.model.clone()),
        json_effort
            .or(selection_effort)
            .or(legacy.model_reasoning_effort_selection.clone()),
        json_small.or(selection_small),
    )
}

fn has_legacy_provider_config(config: &ProviderConfigSection) -> bool {
    // Session defaults (`model`, effort) may live permanently in config.toml.
    // Only provider directory / binding tables count as legacy TOML to migrate.
    config.model_provider.is_some()
        || config.defaults.model_binding.is_some()
        || !config.providers.is_empty()
        || !config.model_bindings.is_empty()
        || !config.model_overrides.is_empty()
        || !config.model_providers.is_empty()
}

fn has_persistable_provider_config(config: &ProviderConfigFile) -> bool {
    !config.providers.is_empty()
}

fn restore_legacy_provider_fields(document: &Value, catalog: &mut ProviderConfigFile) {
    let Some(providers) = document
        .as_table()
        .and_then(|table| table.get("providers"))
        .and_then(Value::as_table)
    else {
        return;
    };
    for (provider_id, provider_value) in providers {
        let Some(provider_table) = provider_value.as_table() else {
            continue;
        };
        let Some(provider) = catalog.providers.get_mut(provider_id) else {
            continue;
        };
        if provider.headers.is_none()
            && let Some(headers) = provider_table.get("headers").and_then(Value::as_str)
            && let Ok(headers) = serde_json::from_str(headers)
        {
            provider.headers = Some(headers);
        }
        if !provider_table.contains_key("enabled") {
            provider.enabled = None;
        }
        for (model_id, model) in &mut provider.models {
            let Some(binding_table) = find_legacy_binding_table(document, provider_id, model_id)
            else {
                continue;
            };
            if !binding_table.contains_key("enabled") {
                model.enabled = None;
            }
            if !binding_table.contains_key("invocation_method") {
                model.wire_api = None;
            }
        }
    }
}

fn find_legacy_binding_table<'a>(
    document: &'a Value,
    provider_id: &str,
    model_id: &str,
) -> Option<&'a toml::map::Map<String, Value>> {
    let bindings = document
        .as_table()
        .and_then(|table| table.get("model_bindings"))
        .and_then(Value::as_table)?;
    bindings.values().find_map(|binding| {
        let binding = binding.as_table()?;
        let binding_provider = binding.get("provider").and_then(Value::as_str)?;
        let request_model = binding
            .get("request_model")
            .or_else(|| binding.get("model_name"))
            .and_then(Value::as_str)?;
        let request_model = request_model
            .strip_prefix(&format!("{provider_id}/"))
            .unwrap_or(request_model);
        (binding_provider == provider_id && request_model == model_id).then_some(binding)
    })
}

fn merge_legacy_model_providers(
    catalog: &mut ProviderConfigFile,
    legacy_providers: &BTreeMap<String, LegacyModelProviderConfig>,
) {
    for (provider_id, legacy_provider) in legacy_providers {
        let provider = catalog
            .providers
            .entry(provider_id.clone())
            .or_insert_with(|| ProviderConfigEntry {
                name: Some(
                    legacy_provider
                        .name
                        .clone()
                        .unwrap_or_else(|| provider_id.clone()),
                ),
                base_url: legacy_provider.base_url.clone(),
                wire_api: Some(
                    legacy_provider
                        .wire_api
                        .unwrap_or(ProviderWireApi::OpenAIChatCompletions),
                ),
                enabled: Some(true),
                ..ProviderConfigEntry::default()
            });
        if provider.name.is_none() {
            provider.name = Some(
                legacy_provider
                    .name
                    .clone()
                    .unwrap_or_else(|| provider_id.clone()),
            );
        }
        if provider.base_url.is_none() {
            provider.base_url = legacy_provider.base_url.clone();
        }
        if provider.wire_api.is_none() {
            provider.wire_api = Some(
                legacy_provider
                    .wire_api
                    .unwrap_or(ProviderWireApi::OpenAIChatCompletions),
            );
        }
        if provider.enabled.is_none() {
            provider.enabled = Some(true);
        }

        let provider_wire_api = provider
            .wire_api
            .unwrap_or(ProviderWireApi::OpenAIChatCompletions);
        for model in &legacy_provider.models {
            let Some(model_id) = legacy_model_id(provider_id, model) else {
                continue;
            };
            let entry = provider.models.entry(model_id).or_default();
            if entry.wire_api.is_none() {
                entry.wire_api = Some(provider_wire_api);
            }
            if provider.base_url.is_none() {
                provider.base_url = model.base_url.clone();
            }
        }
    }
}

fn legacy_model_id(provider_id: &str, model: &ConfiguredModel) -> Option<String> {
    let model_id = model.model.trim();
    if model_id.is_empty() {
        return None;
    }
    Some(
        model_id
            .strip_prefix(&format!("{provider_id}/"))
            .unwrap_or(model_id)
            .to_string(),
    )
}

fn legacy_model_selection(
    legacy: &ProviderConfigSection,
    catalog: &ProviderConfigFile,
) -> Option<String> {
    let provider_id = legacy.model_provider.as_deref().or_else(|| {
        (legacy.model_providers.len() == 1)
            .then(|| legacy.model_providers.keys().next().map(String::as_str))
            .flatten()
    })?;
    let legacy_provider = legacy.model_providers.get(provider_id);
    let model_id = legacy
        .model
        .as_deref()
        .or_else(|| legacy_provider.and_then(|provider| provider.default_model.as_deref()))
        .or_else(|| legacy_provider.and_then(|provider| provider.last_model.as_deref()))?;
    let model_id = model_id
        .strip_prefix(&format!("{provider_id}/"))
        .unwrap_or(model_id)
        .trim();
    if model_id.is_empty() {
        return None;
    }
    catalog
        .providers
        .get(provider_id)
        .map(|_| format!("{provider_id}/{model_id}"))
}

fn migrate_legacy_model_overrides(
    catalog: &mut ProviderConfigFile,
    overrides: &BTreeMap<String, ModelOverrideConfig>,
) -> Result<BTreeSet<String>, ProviderConfigError> {
    let mut consumed = BTreeSet::new();
    for (model_reference, override_config) in overrides {
        let matches = catalog
            .providers
            .iter()
            .flat_map(|(provider_id, provider)| {
                provider.models.keys().filter_map(move |model_id| {
                    let exact_reference = format!("{provider_id}/{model_id}");
                    (model_reference == model_id || model_reference == &exact_reference)
                        .then(|| (provider_id.clone(), model_id.clone()))
                })
            })
            .collect::<Vec<_>>();
        if matches.is_empty() {
            continue;
        }

        let overlay = model_override_as_catalog_model(override_config)?;
        for (provider_id, model_id) in matches {
            if let Some(model) = catalog
                .providers
                .get_mut(&provider_id)
                .and_then(|provider| provider.models.get_mut(&model_id))
            {
                model.apply_overlay(overlay.clone());
            }
        }
        consumed.insert(model_reference.clone());
    }
    Ok(consumed)
}

fn model_override_as_catalog_model(
    override_config: &ModelOverrideConfig,
) -> Result<ProviderModelConfig, ProviderConfigError> {
    let mut value =
        serde_json::to_value(override_config).map_err(|error| ProviderConfigError::Serialize {
            message: error.to_string(),
        })?;
    if let (Some(object), Some(wire_api)) = (value.as_object_mut(), override_config.provider) {
        object.insert(
            "wire_api".to_string(),
            serde_json::to_value(wire_api).map_err(|error| ProviderConfigError::Serialize {
                message: error.to_string(),
            })?,
        );
    }
    serde_json::from_value(value).map_err(|error| ProviderConfigError::Serialize {
        message: error.to_string(),
    })
}

fn migrate_legacy_api_keys(
    document: &Value,
    legacy: &ProviderConfigSection,
    catalog: &mut ProviderConfigFile,
    user_config_dir: &Path,
) -> Result<(), ProviderConfigError> {
    let mut credentials = BTreeMap::new();
    collect_provider_api_keys(document, "providers", &mut credentials);
    collect_provider_api_keys(document, "model_providers", &mut credentials);

    for (provider_id, provider) in &legacy.model_providers {
        if credentials.contains_key(provider_id) {
            continue;
        }
        if let Some(api_key) = provider.api_key.as_deref().and_then(non_empty_string) {
            credentials.insert(provider_id.clone(), api_key);
            continue;
        }
        if let Some(api_key) = provider
            .models
            .iter()
            .find_map(|model| model.api_key.as_deref().and_then(non_empty_string))
        {
            credentials.insert(provider_id.clone(), api_key);
        }
    }

    if credentials.is_empty() {
        return Ok(());
    }

    let auth_file = user_config_dir.join(super::auth::AUTH_CONFIG_FILE_NAME);
    let auth = read_user_auth_config(&auth_file)?;
    let mut existing_credentials = auth.credentials.into_keys().collect::<BTreeSet<_>>();
    for (provider_id, api_key) in credentials {
        let credential_id = legacy
            .providers
            .get(&provider_id)
            .and_then(|provider| provider.credential.clone())
            .unwrap_or_else(|| super::auth::default_provider_credential_id(&provider_id));
        if let Some(provider) = catalog.providers.get_mut(&provider_id) {
            provider.credential = Some(credential_id.clone());
        }
        if existing_credentials.insert(credential_id.clone()) {
            upsert_user_auth_api_key(user_config_dir, &credential_id, &api_key)?;
        }
    }
    Ok(())
}

fn collect_provider_api_keys(
    document: &Value,
    table_name: &str,
    credentials: &mut BTreeMap<String, String>,
) {
    let Some(providers) = document
        .as_table()
        .and_then(|table| table.get(table_name))
        .and_then(Value::as_table)
    else {
        return;
    };
    for (provider_id, provider) in providers {
        let Some(api_key) = provider
            .as_table()
            .and_then(|table| table.get("api_key"))
            .and_then(Value::as_str)
            .and_then(non_empty_string)
        else {
            continue;
        };
        credentials.entry(provider_id.clone()).or_insert(api_key);
    }
}

fn remove_migrated_legacy_config(
    config_file: &Path,
    mut document: Value,
    legacy: &ProviderConfigSection,
    migrated: &ProviderConfigFile,
    consumed_model_overrides: &BTreeSet<String>,
) -> Result<(), ProviderConfigError> {
    let table = ensure_table(&mut document);
    let mut changed = false;
    if !legacy.providers.is_empty() {
        changed |= table.remove("providers").is_some();
    }
    if !legacy.model_bindings.is_empty() {
        changed |= table.remove("model_bindings").is_some();
    }
    if !legacy.model_providers.is_empty() {
        changed |= table.remove("model_providers").is_some();
    }
    if legacy.model_provider.is_some()
        && migrated
            .providers
            .contains_key(legacy.model_provider.as_deref().unwrap_or_default())
    {
        changed |= table.remove("model_provider").is_some();
    }
    // Keep `model` / `model_reasoning_effort_selection` in config.toml as prefs.
    // Promote defaults.model_binding → top-level model when absent.
    if legacy.defaults.model_binding.is_some() {
        let (promoted_binding, defaults_empty) =
            if let Some(defaults) = table.get_mut("defaults").and_then(Value::as_table_mut) {
                let binding = match defaults.remove("model_binding") {
                    Some(Value::String(binding)) => Some(binding),
                    _ => None,
                };
                (binding, defaults.is_empty())
            } else {
                (None, false)
            };
        if let Some(binding) = promoted_binding
            && !table.contains_key("model")
        {
            table.insert("model".to_string(), Value::String(binding));
            changed = true;
        }
        if defaults_empty {
            changed |= table.remove("defaults").is_some();
        }
    }

    if let Some(Value::Table(model_overrides)) = table.get_mut("model") {
        for model_reference in consumed_model_overrides {
            changed |= model_overrides.remove(model_reference).is_some();
        }
        if model_overrides.is_empty() {
            changed |= table.remove("model").is_some();
        }
    }

    if changed {
        let data =
            toml::to_string_pretty(&document).map_err(|error| ProviderConfigError::Serialize {
                message: error.to_string(),
            })?;
        write_atomic(config_file, data.as_bytes())?;
    }
    Ok(())
}

fn ensure_toml_session_defaults(
    config_file: &Path,
    model: Option<String>,
    reasoning_effort: Option<String>,
    small_model: Option<String>,
) -> Result<(), ProviderConfigError> {
    if model.is_none() && reasoning_effort.is_none() && small_model.is_none() {
        return Ok(());
    }
    let mut document = if config_file.exists() {
        read_provider_config_document(config_file)?
    } else {
        Value::Table(Default::default())
    };
    let table = ensure_table(&mut document);
    let mut changed = false;
    if let Some(model) = model {
        // Prefer fully-qualified `provider/model` over a bare legacy model id.
        let replace = match table.get("model").and_then(Value::as_str) {
            None => true,
            Some(existing) => existing != model.as_str(),
        };
        if replace {
            table.insert("model".to_string(), Value::String(model));
            changed = true;
        }
    }
    if let Some(effort) = reasoning_effort
        && !table.contains_key("model_reasoning_effort_selection")
        && !table.contains_key("reasoning_effort")
    {
        table.insert(
            "model_reasoning_effort_selection".to_string(),
            Value::String(effort),
        );
        changed = true;
    }
    if let Some(small_model) = small_model {
        let replace = match table.get("small_model").and_then(Value::as_str) {
            None => true,
            Some(existing) => existing != small_model.as_str(),
        };
        if replace {
            table.insert("small_model".to_string(), Value::String(small_model));
            changed = true;
        }
    }
    if changed {
        let data =
            toml::to_string_pretty(&document).map_err(|error| ProviderConfigError::Serialize {
                message: error.to_string(),
            })?;
        write_atomic(config_file, data.as_bytes())?;
    }
    Ok(())
}

fn ensure_table(value: &mut Value) -> &mut toml::map::Map<String, Value> {
    if !value.is_table() {
        *value = Value::Table(Default::default());
    }
    value
        .as_table_mut()
        .expect("value should be a TOML table after normalization")
}

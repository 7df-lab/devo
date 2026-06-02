use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use devo_protocol::ProviderVendor;
use toml::Value;

use crate::ProviderConfigError;

use super::schema::ProviderConfigSection;
use super::schema::ProviderVendorConfig;

pub const CONFIG_FILE_NAME: &str = "config.toml";

pub(crate) fn read_provider_config(
    config_file: &Path,
) -> Result<ProviderConfigSection, ProviderConfigError> {
    if !config_file.exists() {
        return Ok(ProviderConfigSection::default());
    }

    let data = std::fs::read_to_string(config_file).map_err(|source| ProviderConfigError::Io {
        action: "read",
        path: config_file.to_path_buf(),
        source,
    })?;
    toml::from_str(&data).map_err(|error| ProviderConfigError::ParseTomlFile {
        path: config_file.to_path_buf(),
        message: error.to_string(),
    })
}

pub(crate) fn write_provider_config(
    config_file: &Path,
    config: &ProviderConfigSection,
) -> Result<(), ProviderConfigError> {
    if let Some(parent) = config_file.parent() {
        fs::create_dir_all(parent).map_err(|source| ProviderConfigError::Io {
            action: "create",
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let mut document = read_provider_config_document(config_file)?;
    update_provider_config_document(&mut document, config)?;
    let data =
        toml::to_string_pretty(&document).map_err(|error| ProviderConfigError::Serialize {
            message: error.to_string(),
        })?;
    write_atomic(config_file, data.as_bytes())
}

/// Reads raw TOML so provider writes can preserve unrelated app config sections.
pub(crate) fn read_provider_config_document(
    config_file: &Path,
) -> Result<Value, ProviderConfigError> {
    if !config_file.exists() {
        return Ok(Value::Table(Default::default()));
    }

    let data = fs::read_to_string(config_file).map_err(|source| ProviderConfigError::Io {
        action: "read",
        path: config_file.to_path_buf(),
        source,
    })?;
    toml::from_str(&data).map_err(|error| ProviderConfigError::ParseTomlFile {
        path: config_file.to_path_buf(),
        message: error.to_string(),
    })
}

/// Overlays provider-owned fields onto an existing TOML document.
fn update_provider_config_document(
    document: &mut Value,
    config: &ProviderConfigSection,
) -> Result<(), ProviderConfigError> {
    let replacement =
        toml::Value::try_from(config).map_err(|error| ProviderConfigError::Serialize {
            message: error.to_string(),
        })?;
    let document = ensure_table(document);
    let replacement = replacement
        .as_table()
        .expect("provider config must serialize to a TOML table");

    overlay_optional_key(document, replacement, "model_provider");
    overlay_optional_key(document, replacement, "model");
    overlay_optional_key(document, replacement, "model_thinking_selection");
    overlay_optional_key(document, replacement, "model_auto_compact_token_limit");
    overlay_optional_key(document, replacement, "model_context_window");
    overlay_optional_key(document, replacement, "disable_response_storage");
    overlay_optional_key(document, replacement, "preferred_auth_method");
    overlay_optional_key(document, replacement, "defaults");

    let providers = document
        .entry("providers".to_string())
        .or_insert_with(|| Value::Table(Default::default()));
    let providers = ensure_table(providers);
    let replacement_providers = replacement.get("providers").and_then(Value::as_table);

    for provider_id in config.providers.keys() {
        let provider = providers
            .entry(provider_id.clone())
            .or_insert_with(|| Value::Table(Default::default()));
        let provider = ensure_table(provider);
        let replacement_provider = replacement_providers
            .and_then(|providers| providers.get(provider_id))
            .and_then(Value::as_table);

        if let Some(replacement_provider) = replacement_provider {
            overlay_optional_key(provider, replacement_provider, "name");
            overlay_optional_key(provider, replacement_provider, "base_url");
            overlay_optional_key(provider, replacement_provider, "credential");
            overlay_optional_key(provider, replacement_provider, "wire_apis");
            overlay_optional_key(provider, replacement_provider, "enabled");
        }
    }

    if providers.is_empty() {
        document.remove("providers");
    }

    let model_bindings = document
        .entry("model_bindings".to_string())
        .or_insert_with(|| Value::Table(Default::default()));
    let model_bindings = ensure_table(model_bindings);
    let replacement_bindings = replacement.get("model_bindings").and_then(Value::as_table);

    for binding_id in config.model_bindings.keys() {
        let binding = model_bindings
            .entry(binding_id.clone())
            .or_insert_with(|| Value::Table(Default::default()));
        let binding = ensure_table(binding);
        let replacement_binding = replacement_bindings
            .and_then(|bindings| bindings.get(binding_id))
            .and_then(Value::as_table);

        if let Some(replacement_binding) = replacement_binding {
            overlay_optional_key(binding, replacement_binding, "model_slug");
            overlay_optional_key(binding, replacement_binding, "provider");
            overlay_optional_key(binding, replacement_binding, "model_name");
            overlay_optional_key(binding, replacement_binding, "display_name");
            overlay_optional_key(binding, replacement_binding, "invocation_method");
            overlay_optional_key(binding, replacement_binding, "default_reasoning_effort");
            overlay_optional_key(binding, replacement_binding, "enabled");
        }
    }

    if model_bindings.is_empty() {
        document.remove("model_bindings");
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

fn overlay_optional_key(
    document: &mut toml::map::Map<String, Value>,
    replacement: &toml::map::Map<String, Value>,
    key: &str,
) {
    if let Some(value) = replacement.get(key) {
        document.insert(key.to_string(), value.clone());
    } else {
        document.remove(key);
    }
}

pub(crate) fn write_atomic(path: &Path, data: &[u8]) -> Result<(), ProviderConfigError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(CONFIG_FILE_NAME);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);

    for attempt in 0..16 {
        let temp_path = parent.join(format!(
            ".{file_name}.{}.{}.tmp",
            std::process::id(),
            nanos + attempt
        ));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(mut file) => {
                if let Err(error) = file.write_all(data).and_then(|()| file.sync_all()) {
                    let _ = fs::remove_file(&temp_path);
                    return Err(ProviderConfigError::Io {
                        action: "write",
                        path: path.to_path_buf(),
                        source: error,
                    });
                }
                if let Err(error) = fs::rename(&temp_path, path) {
                    let _ = fs::remove_file(&temp_path);
                    return Err(ProviderConfigError::Io {
                        action: "write",
                        path: path.to_path_buf(),
                        source: error,
                    });
                }
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(ProviderConfigError::Io {
                    action: "create",
                    path: temp_path,
                    source: error,
                });
            }
        }
    }

    Err(ProviderConfigError::Validation {
        message: format!(
            "failed to create temporary config file in {}",
            parent.display()
        ),
    })
}

pub(crate) fn provider_vendor_from_config(
    provider_id: &str,
    provider_config: &ProviderVendorConfig,
) -> ProviderVendor {
    ProviderVendor {
        name: if provider_config.name.is_empty() {
            provider_id.to_string()
        } else {
            provider_config.name.clone()
        },
        base_url: provider_config.base_url.clone(),
        credential: provider_config.credential.clone(),
        wire_apis: provider_config.wire_apis.clone(),
        enabled: provider_config.enabled,
    }
}

pub(crate) fn non_empty_string(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

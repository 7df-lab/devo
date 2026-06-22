//! Provider and model-binding resolution from serialized config plus user auth.
//!
//! This module keeps the TOML/auth shapes separate from runtime settings:
//! callers get owned resolved values, while validation can still distinguish
//! missing, disabled, and unsupported provider/binding states for user-facing
//! errors.

use devo_util_paths::current_user_config_file;

use crate::ProviderConfigError;

use super::auth::current_user_auth_config;
use super::schema::AuthCredentialKind;
use super::schema::ModelBindingConfig;
use super::schema::ProviderConfigSection;
use super::schema::ProviderVendorConfig;
use super::schema::ResolvedProviderSettings;
use super::schema::UserAuthConfigFile;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedModelBinding {
    pub binding_id: String,
    pub model_slug: String,
    pub model_name: String,
    pub provider_id: String,
    pub invocation_method: devo_protocol::ProviderWireApi,
    pub default_reasoning_effort: Option<String>,
    pub enabled: bool,
}

impl ResolvedModelBinding {
    fn from_config(binding_id: &str, binding: &ModelBindingConfig) -> Self {
        Self {
            binding_id: binding_id.to_string(),
            model_slug: binding.model_slug.clone(),
            model_name: binding.model_name.clone(),
            provider_id: binding.provider.clone(),
            invocation_method: binding.invocation_method,
            default_reasoning_effort: binding.default_reasoning_effort.clone(),
            enabled: binding.enabled,
        }
    }
}

#[derive(Clone, Copy)]
enum BindingVisibility {
    IncludeDisabled,
    EnabledOnly,
}

impl BindingVisibility {
    fn allows(self, binding: &ModelBindingConfig) -> bool {
        match self {
            Self::IncludeDisabled => true,
            Self::EnabledOnly => binding.enabled,
        }
    }
}

/// Loads the user's provider config file from the standard config path.
pub fn load_config() -> Result<ProviderConfigSection, ProviderConfigError> {
    let path = current_user_config_file().map_err(|error| ProviderConfigError::ConfigPath {
        message: format!("could not determine user config path: {error}"),
    })?;
    if path.exists() {
        let data = std::fs::read_to_string(&path).map_err(|source| ProviderConfigError::Io {
            action: "read",
            path: path.clone(),
            source,
        })?;
        return toml::from_str(&data).map_err(|error| ProviderConfigError::ParseTomlFile {
            path,
            message: error.to_string(),
        });
    }

    Ok(ProviderConfigSection::default())
}

/// Resolves provider settings without constructing a local provider instance.
pub fn resolve_provider_settings() -> Result<ResolvedProviderSettings, ProviderConfigError> {
    let auth = current_user_auth_config()?;
    resolve_provider_settings_from_config_and_auth(&load_config().unwrap_or_default(), &auth)
}

/// Resolves provider settings using user-scoped auth material for new providers.
pub fn resolve_provider_settings_from_config_and_auth(
    file: &ProviderConfigSection,
    auth: &UserAuthConfigFile,
) -> Result<ResolvedProviderSettings, ProviderConfigError> {
    if let Some(binding) = resolve_model_binding(file, None) {
        let provider_config = file.providers.get(&binding.provider_id).ok_or_else(|| {
            ProviderConfigError::Validation {
                message: format!(
                    "configured provider `{}` was not found",
                    binding.provider_id
                ),
            }
        })?;
        if !provider_config.enabled {
            return Err(ProviderConfigError::Validation {
                message: format!("configured provider `{}` is disabled", binding.provider_id),
            });
        }
        if !binding.enabled {
            return Err(ProviderConfigError::Validation {
                message: format!(
                    "configured model binding `{}` is disabled",
                    binding.binding_id
                ),
            });
        }
        if !provider_config.wire_apis.is_empty()
            && !provider_config
                .wire_apis
                .contains(&binding.invocation_method)
        {
            return Err(ProviderConfigError::Validation {
                message: format!(
                    "model binding `{}` uses unsupported provider wire API `{}`",
                    binding.binding_id, binding.invocation_method
                ),
            });
        }

        let api_key = resolve_provider_api_key(&binding.provider_id, provider_config, auth)?;
        let model_reasoning_effort_selection = file
            .model_reasoning_effort_selection
            .clone()
            .or(binding.default_reasoning_effort);

        return Ok(ResolvedProviderSettings {
            provider_id: binding.provider_id,
            wire_api: binding.invocation_method,
            model: binding.model_name,
            base_url: provider_config.base_url.clone(),
            api_key,
            proxy_url: None,
            headers: provider_config.headers.clone(),
            model_auto_compact_token_limit: file.model_auto_compact_token_limit,
            model_context_window: file.model_context_window,
            model_reasoning_effort_selection,
            disable_response_storage: file.disable_response_storage.unwrap_or(false),
            preferred_auth_method: file.preferred_auth_method,
        });
    }

    Err(ProviderConfigError::Validation {
        message: "No provider configured. Run `devo onboard` to complete setup.".to_string(),
    })
}

fn resolve_provider_api_key(
    provider_id: &str,
    provider_config: &ProviderVendorConfig,
    auth: &UserAuthConfigFile,
) -> Result<Option<String>, ProviderConfigError> {
    let Some(credential_id) = provider_config.credential.as_deref() else {
        return Ok(None);
    };
    let credential =
        auth.credentials
            .get(credential_id)
            .ok_or_else(|| ProviderConfigError::Validation {
                message: format!(
                    "provider `{provider_id}` references missing credential `{credential_id}` in user auth.json"
                ),
            })?;
    match credential.kind {
        AuthCredentialKind::ApiKey => Ok(Some(credential.value.clone())),
    }
}

pub fn resolve_model_binding(
    config: &ProviderConfigSection,
    requested_model: Option<&str>,
) -> Option<ResolvedModelBinding> {
    // This resolver is used for configuration validation. It intentionally keeps
    // a configured-but-disabled binding visible so the caller can report
    // "binding is disabled" instead of silently selecting another model.
    if let Some(requested_model) = requested_model {
        return requested_model_binding(
            config,
            requested_model,
            BindingVisibility::IncludeDisabled,
        );
    }

    if let Some(binding) = config
        .defaults
        .model_binding
        .as_deref()
        .and_then(|binding_id| {
            config
                .model_bindings
                .get(binding_id)
                .map(|binding| ResolvedModelBinding::from_config(binding_id, binding))
        })
    {
        return Some(binding);
    }

    config
        .model
        .as_deref()
        .and_then(|model| resolve_model_binding(config, Some(model)))
        .or_else(|| {
            config
                .model_bindings
                .iter()
                .find(|(_, binding)| binding.enabled)
                .map(|(binding_id, binding)| ResolvedModelBinding::from_config(binding_id, binding))
        })
}

pub fn resolve_enabled_model_binding(
    config: &ProviderConfigSection,
    requested_model: Option<&str>,
) -> Option<ResolvedModelBinding> {
    // Runtime turn selection only uses enabled bindings. A user-facing model
    // override may name the binding id, the local catalog slug, or the provider
    // wire name: e.g. `deepseek-main`, `deepseek-v4-pro`, or
    // `deepseek/deepseek-v4-pro`.
    if let Some(requested_model) = requested_model {
        return requested_model_binding(config, requested_model, BindingVisibility::EnabledOnly);
    }

    config
        .defaults
        .model_binding
        .as_deref()
        .and_then(|binding_id| {
            config
                .model_bindings
                .get(binding_id)
                .filter(|binding| binding.enabled)
                .map(|binding| ResolvedModelBinding::from_config(binding_id, binding))
        })
        .or_else(|| {
            config
                .model_bindings
                .iter()
                .find(|(_, binding)| binding.enabled)
                .map(|(binding_id, binding)| ResolvedModelBinding::from_config(binding_id, binding))
        })
}

fn requested_model_binding(
    config: &ProviderConfigSection,
    requested_model: &str,
    visibility: BindingVisibility,
) -> Option<ResolvedModelBinding> {
    config
        .model_bindings
        .iter()
        .find(|(binding_id, binding)| {
            visibility.allows(binding) && binding_id.as_str() == requested_model
        })
        .or_else(|| {
            config.model_bindings.iter().find(|(_, binding)| {
                visibility.allows(binding)
                    && (binding.model_slug == requested_model
                        || binding.model_name == requested_model)
            })
        })
        .map(|(binding_id, binding)| ResolvedModelBinding::from_config(binding_id, binding))
}

pub fn provider_request_model_map_for_binding(
    config: &ProviderConfigSection,
    binding: &ResolvedModelBinding,
) -> std::collections::HashMap<String, String> {
    // Reasoning model variants are catalog slugs first. When `kimi-k2.5` resolves
    // to variant slug `kimi-k2.5-thinking`, the provider request must use the
    // matching binding's `model_name`, such as `moonshotai/kimi-k2.5-thinking`.
    // Scope this map to the selected provider so another provider with the same
    // variant slug cannot hijack the wire model name.
    let mut request_model_map =
        std::collections::HashMap::with_capacity(config.model_bindings.len());
    for candidate in config
        .model_bindings
        .values()
        .filter(|candidate| candidate.enabled && candidate.provider == binding.provider_id)
    {
        request_model_map.insert(candidate.model_slug.clone(), candidate.model_name.clone());
    }
    request_model_map
}

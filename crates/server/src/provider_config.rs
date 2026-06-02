use anyhow::Context;
use anyhow::Result;

use devo_core::AppConfig;
use devo_core::LegacyModelProviderConfig;
use devo_core::ModelCatalog;
use devo_core::PresetModelCatalog;
use devo_core::ProviderConfigSection;
use devo_core::ProviderWireApi;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::StreamEvent;
use devo_provider::ModelProviderSDK;
use devo_provider::anthropic::AnthropicProvider;
use devo_provider::openai::OpenAIProvider;
use devo_provider::openai::OpenAIResponsesProvider;
use std::path::Path;
use std::pin::Pin;

const NO_PROVIDER_CONFIGURED_MESSAGE: &str =
    "No provider configured. Run `devo onboard` to complete setup.";

/// Resolved provider bootstrap owned by the server runtime.
pub struct ResolvedServerProvider {
    /// Concrete provider used for model requests.
    pub provider: std::sync::Arc<dyn ModelProviderSDK>,
    /// Default model slug used when a session or turn does not request one.
    pub default_model: String,
}

/// Loads the server-side provider from a merged app config.
pub fn load_server_provider(
    app_config: &AppConfig,
    default_model: Option<&str>,
    user_config_dir: &Path,
) -> Result<ResolvedServerProvider> {
    if !app_config.has_provider_configuration() {
        let default_model = match default_model {
            Some(default_model) => default_model.to_string(),
            None => PresetModelCatalog::load()?
                .resolve_for_turn(None)?
                .slug
                .clone(),
        };
        return Ok(ResolvedServerProvider {
            provider: std::sync::Arc::new(MissingProvider),
            default_model,
        });
    }

    if app_config.provider.model_providers.is_empty() {
        let resolved = app_config.resolve_provider_settings(user_config_dir)?;
        return build_server_provider(
            resolved.wire_api,
            resolved.model,
            resolved.base_url,
            resolved.api_key,
        );
    }

    load_legacy_server_provider(app_config, default_model)
}

struct MissingProvider;

#[async_trait::async_trait]
impl ModelProviderSDK for MissingProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        anyhow::bail!(NO_PROVIDER_CONFIGURED_MESSAGE)
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        anyhow::bail!(NO_PROVIDER_CONFIGURED_MESSAGE)
    }

    fn name(&self) -> &str {
        "missing-provider"
    }
}

fn load_legacy_server_provider(
    app_config: &AppConfig,
    default_model: Option<&str>,
) -> Result<ResolvedServerProvider> {
    let resolved = resolve_legacy_server_provider_settings(&app_config.provider, default_model)?;
    build_server_provider(
        resolved.wire_api,
        resolved.model,
        resolved.base_url,
        resolved.api_key,
    )
}

fn build_server_provider(
    wire_api: ProviderWireApi,
    model: String,
    base_url: Option<String>,
    api_key: Option<String>,
) -> Result<ResolvedServerProvider> {
    let provider: std::sync::Arc<dyn ModelProviderSDK> = match wire_api {
        ProviderWireApi::AnthropicMessages => {
            let api_key = api_key.context("anthropic provider requires an API key")?;
            let base_url = base_url.unwrap_or_else(|| "https://api.anthropic.com".to_string());
            std::sync::Arc::new(AnthropicProvider::new(base_url).with_api_key(api_key))
        }
        ProviderWireApi::OpenAIChatCompletions => {
            let base_url = normalize_openai_base_url(
                &base_url.unwrap_or_else(|| "https://api.openai.com".to_string()),
            );
            let mut provider = OpenAIProvider::new(base_url);
            if let Some(api_key) = api_key {
                provider = provider.with_api_key(api_key);
            }
            std::sync::Arc::new(provider)
        }
        ProviderWireApi::OpenAIResponses => {
            let base_url = normalize_openai_base_url(
                &base_url.unwrap_or_else(|| "https://api.openai.com".to_string()),
            );
            let mut provider = OpenAIResponsesProvider::new(base_url);
            if let Some(api_key) = api_key {
                provider = provider.with_api_key(api_key);
            }
            std::sync::Arc::new(provider)
        }
    };

    Ok(ResolvedServerProvider {
        provider,
        default_model: model,
    })
}

#[derive(Debug, PartialEq, Eq)]
struct ServerProviderSettings {
    wire_api: ProviderWireApi,
    model: String,
    base_url: Option<String>,
    api_key: Option<String>,
}

#[cfg(test)]
fn resolve_server_provider_settings(
    file_config: &ProviderConfigSection,
    default_model: Option<&str>,
    auth: &devo_core::UserAuthConfigFile,
) -> Result<ServerProviderSettings> {
    if let Some(binding) = active_model_binding(file_config) {
        let provider = file_config
            .providers
            .get(&binding.provider)
            .with_context(|| format!("configured provider `{}` was not found", binding.provider))?;
        if !provider.enabled {
            anyhow::bail!("configured provider `{}` is disabled", binding.provider);
        }
        if !provider.wire_apis.is_empty()
            && !provider.wire_apis.contains(&binding.invocation_method)
        {
            anyhow::bail!(
                "model binding `{}` uses unsupported provider wire API `{}`",
                binding.model_slug,
                binding.invocation_method
            );
        }
        return Ok(ServerProviderSettings {
            wire_api: binding.invocation_method,
            model: binding.model_name.clone(),
            base_url: provider.base_url.clone(),
            api_key: resolve_provider_api_key(&binding.provider, provider, auth)?,
        });
    }

    resolve_legacy_server_provider_settings(file_config, default_model)
}

#[cfg(test)]
fn resolve_provider_api_key(
    provider_id: &str,
    provider: &devo_core::ProviderVendorConfig,
    auth: &devo_core::UserAuthConfigFile,
) -> Result<Option<String>> {
    let Some(credential_id) = provider.credential.as_deref() else {
        return Ok(None);
    };
    let credential = auth.credentials.get(credential_id).with_context(|| {
        format!(
            "provider `{provider_id}` references missing credential `{credential_id}` in user auth.json"
        )
    })?;
    Ok(Some(credential.value.clone()))
}

#[cfg(test)]
fn active_model_binding(config: &ProviderConfigSection) -> Option<&devo_core::ModelBindingConfig> {
    config
        .defaults
        .model_binding
        .as_deref()
        .and_then(|binding_id| config.model_bindings.get(binding_id))
        .or_else(|| {
            config.model.as_deref().and_then(|model| {
                config
                    .model_bindings
                    .values()
                    .find(|binding| binding.model_slug == model || binding.model_name == model)
            })
        })
        .or_else(|| {
            config
                .model_bindings
                .values()
                .find(|binding| binding.enabled)
        })
}

fn resolve_legacy_server_provider_settings(
    file_config: &ProviderConfigSection,
    default_model: Option<&str>,
) -> Result<ServerProviderSettings> {
    let requested_model = file_config.model.as_deref();
    let provider_id = provider_id_for_model(file_config, requested_model)
        .or_else(|| {
            file_config
                .model_provider
                .clone()
                .filter(|provider| file_config.model_providers.contains_key(provider))
        })
        .or_else(|| file_config.model_providers.keys().next().cloned());
    let provider_config = provider_id
        .as_deref()
        .and_then(|provider_id| file_config.model_providers.get(provider_id));
    let selected_model =
        provider_config.and_then(|provider| select_configured_model(provider, requested_model));
    let wire_api = provider_config
        .and_then(|provider| provider.wire_api)
        .unwrap_or(ProviderWireApi::OpenAIChatCompletions);
    let model = selected_model
        .map(|model| model.model.clone())
        .or(file_config.model.clone())
        .or_else(|| default_model.map(ToOwned::to_owned))
        .or_else(|| provider_config.and_then(|provider| provider.default_model.clone()))
        .or_else(|| {
            provider_config
                .and_then(|provider| provider.models.first().map(|model| model.model.clone()))
        })
        .context("no model configured for server provider")?;
    let base_url = selected_model
        .and_then(|model| model.base_url.clone())
        .or_else(|| provider_config.and_then(|provider| provider.base_url.clone()));
    let api_key = selected_model
        .and_then(|model| model.api_key.clone())
        .or_else(|| provider_config.and_then(|provider| provider.api_key.clone()));

    Ok(ServerProviderSettings {
        wire_api,
        model,
        base_url,
        api_key,
    })
}

fn select_configured_model<'a>(
    profile: &'a LegacyModelProviderConfig,
    requested: Option<&str>,
) -> Option<&'a devo_core::ConfiguredModel> {
    match requested {
        Some(model) => profile.models.iter().find(|entry| entry.model == model),
        None => profile
            .default_model
            .as_deref()
            .and_then(|default| profile.models.iter().find(|entry| entry.model == default))
            .or_else(|| profile.models.first()),
    }
}

fn provider_id_for_model(
    config: &ProviderConfigSection,
    requested_model: Option<&str>,
) -> Option<String> {
    let requested_model = requested_model?;
    config
        .model_providers
        .iter()
        .find(|(_, provider)| {
            provider.last_model.as_deref() == Some(requested_model)
                || provider.default_model.as_deref() == Some(requested_model)
                || provider
                    .models
                    .iter()
                    .any(|entry| entry.model == requested_model)
        })
        .map(|(provider_id, _)| provider_id.clone())
}

pub(crate) fn normalize_openai_base_url(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    let Some(scheme_sep) = trimmed.find("://") else {
        return trimmed.to_string();
    };
    let has_explicit_path = trimmed[scheme_sep + 3..].contains('/');
    if has_explicit_path {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v1")
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use devo_core::AuthCredentialConfig;
    use devo_core::AuthCredentialKind;
    use devo_core::ModelBindingConfig;
    use devo_core::ProviderConfigSection;
    use devo_core::ProviderDefaultsConfig;
    use devo_core::ProviderVendorConfig;
    use devo_core::UserAuthConfigFile;
    use pretty_assertions::assert_eq;

    use super::load_server_provider;
    use super::normalize_openai_base_url;
    use super::resolve_server_provider_settings;
    use devo_protocol::ProviderWireApi;

    #[test]
    fn preserves_explicit_openai_compatible_paths() {
        assert_eq!(
            normalize_openai_base_url("https://open.bigmodel.cn/api/paas/v4/"),
            "https://open.bigmodel.cn/api/paas/v4"
        );
    }

    #[test]
    fn appends_v1_for_bare_openai_hosts() {
        assert_eq!(
            normalize_openai_base_url("https://api.openai.com"),
            "https://api.openai.com/v1"
        );
    }

    #[tokio::test]
    async fn empty_provider_config_loads_missing_provider_for_onboarding() {
        let config = devo_core::AppConfig::default();
        let dir = tempfile::tempdir().expect("temp dir");

        let actual = load_server_provider(&config, Some("onboard-model"), dir.path())
            .expect("load missing provider");

        assert_eq!(actual.default_model, "onboard-model");
        assert_eq!(actual.provider.name(), "missing-provider");
        let error = actual
            .provider
            .completion(devo_protocol::ModelRequest {
                model: "onboard-model".to_string(),
                system: None,
                messages: Vec::new(),
                max_tokens: 1,
                tools: None,
                sampling: devo_protocol::SamplingControls::default(),
                thinking: None,
                reasoning_effort: None,
                extra_body: None,
            })
            .await
            .expect_err("missing provider should reject model requests");

        assert_eq!(
            error.to_string(),
            "No provider configured. Run `devo onboard` to complete setup."
        );
    }

    #[test]
    fn resolves_provider_credential_id_through_user_auth() {
        let config = ProviderConfigSection {
            defaults: ProviderDefaultsConfig {
                model_binding: Some("gpt-test-openrouter".to_string()),
            },
            providers: BTreeMap::from([(
                "openrouter".to_string(),
                ProviderVendorConfig {
                    name: "openrouter".to_string(),
                    credential: Some("openrouter_api_key".to_string()),
                    wire_apis: vec![ProviderWireApi::OpenAIResponses],
                    enabled: true,
                    ..ProviderVendorConfig::default()
                },
            )]),
            model_bindings: BTreeMap::from([(
                "gpt-test-openrouter".to_string(),
                ModelBindingConfig {
                    model_slug: "gpt-test".to_string(),
                    provider: "openrouter".to_string(),
                    model_name: "openai/gpt-test".to_string(),
                    invocation_method: ProviderWireApi::OpenAIResponses,
                    ..ModelBindingConfig::default()
                },
            )]),
            ..ProviderConfigSection::default()
        };
        let auth = UserAuthConfigFile {
            credentials: BTreeMap::from([(
                "openrouter_api_key".to_string(),
                AuthCredentialConfig {
                    kind: AuthCredentialKind::ApiKey,
                    value: "sk-or-secret".to_string(),
                },
            )]),
            ..UserAuthConfigFile::default()
        };

        let actual = resolve_server_provider_settings(&config, None, &auth)
            .expect("resolve server provider settings");

        assert_eq!(
            actual,
            super::ServerProviderSettings {
                wire_api: ProviderWireApi::OpenAIResponses,
                model: "openai/gpt-test".to_string(),
                base_url: None,
                api_key: Some("sk-or-secret".to_string()),
            }
        );
    }
}

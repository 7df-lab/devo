//! Credential bindings and transport initialization for explicit provider login.
//! Background credential synchronization must never infer a new route.

use std::path::{Path, PathBuf};

use devo_core::{ProviderConfigEntry, ProviderWireApi};

#[derive(Clone, Copy)]
pub(super) struct OAuthTransport {
    endpoint: &'static str,
    protocol: ProviderWireApi,
}

pub(super) fn oauth_transport_defaults(provider_id: &str) -> Option<OAuthTransport> {
    let (endpoint, protocol) = match provider_id {
        "openai-codex" => (
            "https://chatgpt.com/backend-api/codex",
            ProviderWireApi::OpenAIResponses,
        ),
        "anthropic" => (
            "https://api.anthropic.com",
            ProviderWireApi::AnthropicMessages,
        ),
        "github-copilot" => (
            "https://api.individual.githubcopilot.com",
            ProviderWireApi::OpenAIChatCompletions,
        ),
        "xai" => (
            "https://api.x.ai/v1",
            ProviderWireApi::OpenAIChatCompletions,
        ),
        _ => return None,
    };
    Some(OAuthTransport { endpoint, protocol })
}

pub(super) enum CredentialBinding {
    Sync,
    OAuth(OAuthTransport),
    CatalogTemplate,
}

pub(super) struct PreparedCredentialBinding {
    file: PathBuf,
    config: devo_core::ProviderConfigFile,
}

impl PreparedCredentialBinding {
    pub(super) fn persist(self) -> Result<(), String> {
        devo_core::write_provider_catalog_config(&self.file, &self.config)
            .map_err(|error| error.to_string())
    }
}

/// Prepare a patch to the owning file, without writing the public model directory.
pub(super) fn prepare_provider_credential_binding(
    user_dir: &Path,
    provider_id: &str,
    credential_id: &str,
    binding: CredentialBinding,
) -> Result<PreparedCredentialBinding, String> {
    let config_file = user_dir.join(devo_core::PROVIDER_CONFIG_FILE_NAME);
    let custom_file = user_dir.join(devo_core::CUSTOM_PROVIDER_CONFIG_FILE_NAME);
    let mut config =
        devo_core::read_provider_catalog_config(&config_file).map_err(|error| error.to_string())?;
    let mut custom =
        devo_core::read_provider_catalog_config(&custom_file).map_err(|error| error.to_string())?;
    // Custom connections take precedence over providers.json in the effective
    // catalog. Do not update a shadowed credential binding.
    let (file, owner, inherited) = if custom.providers.contains_key(provider_id) {
        (custom_file, &mut custom, config.providers.get(provider_id))
    } else {
        (config_file, &mut config, None)
    };
    let entry = owner
        .providers
        .entry(provider_id.to_string())
        .or_insert_with(|| ProviderConfigEntry {
            name: Some(provider_id.to_string()),
            ..Default::default()
        });
    entry.credential = Some(credential_id.to_string());
    entry
        .enabled
        .get_or_insert_with(|| inherited.and_then(|entry| entry.enabled).unwrap_or(true));
    let endpoint = entry
        .base_url
        .as_deref()
        .or_else(|| inherited.and_then(|entry| entry.base_url.as_deref()));
    let has_endpoint = endpoint.is_some_and(|endpoint| !endpoint.trim().is_empty());
    let wire_api = entry
        .wire_api
        .or_else(|| inherited.and_then(|entry| entry.wire_api));
    match binding {
        CredentialBinding::Sync => {}
        CredentialBinding::OAuth(transport) => {
            if !has_endpoint {
                entry.base_url = Some(transport.endpoint.to_string());
            }
            if wire_api.is_none() {
                entry.wire_api = Some(transport.protocol);
            }
        }
        CredentialBinding::CatalogTemplate => {
            let has_model_endpoint = entry
                .models
                .values()
                .chain(
                    inherited
                        .into_iter()
                        .flat_map(|entry| entry.models.values()),
                )
                .any(|model| {
                    model
                        .base_url
                        .as_deref()
                        .is_some_and(|endpoint| !endpoint.trim().is_empty())
                });
            if !has_endpoint && !has_model_endpoint {
                // The effective directory intentionally strips remote routes
                // after a local entry exists. Read the template only on explicit
                // login so a sparse legacy entry can be repaired as well.
                let catalog = devo_core::load_remote_catalog_overlay(user_dir)
                    .map_err(|error| error.to_string())?;
                let route = catalog.as_ref().and_then(|catalog| {
                    let template = catalog.providers.get(provider_id)?;
                    Some((template.base_url.as_deref()?, template.wire_api?))
                });
                let Some((endpoint, protocol)) = route.filter(|(endpoint, protocol)| {
                    wire_api.is_none_or(|saved| saved == *protocol)
                        && reqwest::Url::parse(endpoint).is_ok_and(|url| {
                            matches!(url.scheme(), "http" | "https") && url.host_str().is_some()
                        })
                }) else {
                    return Err(format!(
                        "provider `{provider_id}` has no compatible catalog endpoint; configure its base URL and wire API explicitly"
                    ));
                };
                entry.base_url = Some(endpoint.to_string());
                if wire_api.is_none() {
                    entry.wire_api = Some(protocol);
                }
            }
        }
    }
    Ok(PreparedCredentialBinding {
        file,
        config: std::mem::take(owner),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn explicit_oauth_connection_sets_only_missing_transport() {
        let home = tempfile::tempdir().unwrap();
        prepare_provider_credential_binding(
            home.path(),
            "openai-codex",
            "token",
            CredentialBinding::OAuth(oauth_transport_defaults("openai-codex").unwrap()),
        )
        .unwrap()
        .persist()
        .unwrap();
        let stored = devo_core::read_provider_catalog_config(
            &home.path().join(devo_core::PROVIDER_CONFIG_FILE_NAME),
        )
        .unwrap();
        let expected = serde_json::from_value(json!({"provider": {"openai-codex": {
            "name": "openai-codex", "credential": "token", "enabled": true,
            "base_url": "https://chatgpt.com/backend-api/codex", "wire_api": "openai_responses"
        }}}))
        .unwrap();
        assert_eq!(stored, expected);
    }

    #[test]
    fn ordinary_oauth_sync_leaves_missing_transport_unpinned() {
        let home = tempfile::tempdir().unwrap();
        prepare_provider_credential_binding(
            home.path(),
            "openai-codex",
            "token",
            CredentialBinding::Sync,
        )
        .unwrap()
        .persist()
        .unwrap();
        let stored = devo_core::read_provider_catalog_config(
            &home.path().join(devo_core::PROVIDER_CONFIG_FILE_NAME),
        )
        .unwrap();
        let expected = serde_json::from_value(json!({"provider": {"openai-codex": {
            "name": "openai-codex", "credential": "token", "enabled": true
        }}}))
        .unwrap();
        assert_eq!(stored, expected);
    }

    #[test]
    fn explicit_login_preserves_custom_endpoint_protocol_models_and_owning_file() {
        let home = tempfile::tempdir().unwrap();
        let path = home
            .path()
            .join(devo_core::CUSTOM_PROVIDER_CONFIG_FILE_NAME);
        let mut expected: devo_core::ProviderConfigFile = serde_json::from_value(json!({"provider": {"anthropic": {
            "base_url": "https://user.example/messages", "wire_api": "openai_responses",
            "enabled": false, "headers": {"x-user": "custom"}, "models": {"my-model": {"context_window": 1234}}
        }}})).unwrap();
        devo_core::write_provider_catalog_config(&path, &expected).unwrap();
        prepare_provider_credential_binding(
            home.path(),
            "anthropic",
            "new-token",
            CredentialBinding::OAuth(oauth_transport_defaults("anthropic").unwrap()),
        )
        .unwrap()
        .persist()
        .unwrap();
        expected.providers.get_mut("anthropic").unwrap().credential = Some("new-token".to_string());
        assert_eq!(
            devo_core::read_provider_catalog_config(&path).unwrap(),
            expected
        );
        assert!(
            !home
                .path()
                .join(devo_core::PROVIDER_CONFIG_FILE_NAME)
                .exists()
        );
    }
    #[tokio::test]
    async fn api_key_login_pins_catalog_route_after_credential_sync() {
        let home = tempfile::tempdir().unwrap();
        let source = home.path().join("catalog.json");
        std::fs::write(&source, json!({"deepseek": {
            "name": "DeepSeek", "api": "https://api.deepseek.com", "npm": "@ai-sdk/openai-compatible",
            "models": {"deepseek-flash": {"name": "DeepSeek Flash"}, "other": {"name": "Other"}}
        }}).to_string()).unwrap();
        let runtime = crate::test_support::TestRuntime::noop().runtime(home.path());
        let refresh = devo_core::refresh_remote_catalog_force(
            home.path(),
            &devo_core::CatalogConfig {
                source: source.to_string_lossy().into_owned(),
                offline: true,
                ..Default::default()
            },
        )
        .await;
        assert_eq!(
            refresh,
            devo_core::CatalogRefreshOutcome::Updated {
                providers: 1,
                models: 2
            }
        );
        let path = home.path().join(devo_core::PROVIDER_CONFIG_FILE_NAME);
        let mut request =
            json!({"provider": "deepseek", "kind": "api_key", "secret": "fixture-key"});
        let synced = runtime
            .handle_native_credential_set(json!(1), request.clone())
            .await;
        assert!(synced.get("error").is_none(), "sync failed: {synced}");
        let mut expected: devo_core::ProviderConfigFile =
            serde_json::from_value(json!({"provider": {"deepseek": {
                "name": "deepseek", "credential": "deepseek", "enabled": true
            }}}))
            .unwrap();
        assert_eq!(
            devo_core::read_provider_catalog_config(&path).unwrap(),
            expected
        );
        request["configureConnection"] = json!(true);
        let connected = runtime
            .handle_native_credential_set(json!(2), request)
            .await;
        assert!(
            connected.get("error").is_none(),
            "connect failed: {connected}"
        );
        let entry = expected.providers.get_mut("deepseek").unwrap();
        entry.base_url = Some("https://api.deepseek.com".to_string());
        entry.wire_api = Some(ProviderWireApi::OpenAIChatCompletions);
        assert_eq!(
            devo_core::read_provider_catalog_config(&path).unwrap(),
            expected
        );

        let selected = runtime.handle_native_provider_upsert(json!(3), json!({
            "defaultModel": "deepseek/deepseek-flash",
            "provider": {
                "id": "deepseek", "name": "deepseek", "credential": "deepseek", "enabled": true,
                "baseUrl": "https://api.deepseek.com", "wireApis": ["openai_chat_completions"],
                "models": {
                    "deepseek-flash": {"origin": "remote", "name": "Remote label", "baseUrl": "https://api.deepseek.com", "wireApi": "openai_chat_completions", "requestModel": "deepseek-flash"},
                    "other": {"origin": "remote", "name": "Unselected model"}
                }
            }
        })).await;
        assert!(
            selected.get("error").is_none(),
            "selection failed: {selected}"
        );
        expected.providers.get_mut("deepseek").unwrap().models = serde_json::from_value(json!({
            "deepseek-flash": {"origin": "remote", "base_url": "https://api.deepseek.com", "wire_api": "openai_chat_completions", "request_model": "deepseek-flash"}
        })).unwrap();
        assert_eq!(
            devo_core::read_provider_catalog_config(&path).unwrap(),
            expected
        );
        let offline = devo_core::effective_provider_catalog(&expected).unwrap();
        assert!(
            offline
                .resolve_model(Some("deepseek/deepseek-flash"))
                .is_ok()
        );
    }

    #[test]
    fn explicit_login_updates_the_effective_custom_binding_without_redirecting_it() {
        let home = tempfile::tempdir().unwrap();
        let connection_path = home.path().join(devo_core::PROVIDER_CONFIG_FILE_NAME);
        let custom_path = home
            .path()
            .join(devo_core::CUSTOM_PROVIDER_CONFIG_FILE_NAME);
        let connection: devo_core::ProviderConfigFile = serde_json::from_value(json!({"provider": {"openai-codex": {
            "credential": "old-standard", "base_url": "https://standard.example", "wire_api": "openai_responses"
        }}})).unwrap();
        let mut custom: devo_core::ProviderConfigFile = serde_json::from_value(json!({"provider": {"openai-codex": {
            "credential": "old-custom", "base_url": "https://custom.example", "wire_api": "openai_responses", "enabled": true
        }}})).unwrap();
        devo_core::write_provider_catalog_config(&connection_path, &connection).unwrap();
        devo_core::write_provider_catalog_config(&custom_path, &custom).unwrap();
        prepare_provider_credential_binding(
            home.path(),
            "openai-codex",
            "new-token",
            CredentialBinding::OAuth(oauth_transport_defaults("openai-codex").unwrap()),
        )
        .unwrap()
        .persist()
        .unwrap();
        custom.providers.get_mut("openai-codex").unwrap().credential =
            Some("new-token".to_string());
        assert_eq!(
            (
                devo_core::read_provider_catalog_config(&connection_path).unwrap(),
                devo_core::read_provider_catalog_config(&custom_path).unwrap(),
            ),
            (connection, custom)
        );
    }

    #[tokio::test]
    async fn credential_set_requires_explicit_oauth_connect_before_pinning() {
        let home = tempfile::tempdir().unwrap();
        let runtime = crate::test_support::TestRuntime::noop().runtime(home.path());
        let mut request = json!({"provider": "openai-codex", "kind": "oauth", "access": "fixture-token",
            "expiresAt": chrono::Utc::now().timestamp() + 86400});
        let synced = runtime
            .handle_native_credential_set(json!(1), request.clone())
            .await;
        assert!(synced.get("error").is_none(), "sync failed: {synced}");
        let path = home.path().join(devo_core::PROVIDER_CONFIG_FILE_NAME);
        let before = devo_core::read_provider_catalog_config(&path).unwrap();
        assert_eq!(
            (
                before.providers["openai-codex"].base_url.clone(),
                before.providers["openai-codex"].wire_api
            ),
            (None, None)
        );
        request["configureConnection"] = json!(true);
        let connected = runtime
            .handle_native_credential_set(json!(2), request)
            .await;
        assert!(
            connected.get("error").is_none(),
            "connect failed: {connected}"
        );
        let mut expected = before;
        let entry = expected.providers.get_mut("openai-codex").unwrap();
        entry.base_url = Some("https://chatgpt.com/backend-api/codex".to_string());
        entry.wire_api = Some(ProviderWireApi::OpenAIResponses);
        assert_eq!(
            devo_core::read_provider_catalog_config(&path).unwrap(),
            expected
        );
    }

    #[tokio::test]
    async fn configure_connection_rejects_missing_templates_and_unknown_adapters_before_writing() {
        let home = tempfile::tempdir().unwrap();
        let runtime = crate::test_support::TestRuntime::noop().runtime(home.path());
        for request in [
            json!({"provider": "anthropic", "kind": "api_key", "secret": "fixture", "configureConnection": true}),
            json!({"provider": "unknown", "kind": "oauth", "access": "fixture", "configureConnection": true}),
        ] {
            let response = runtime
                .handle_native_credential_set(json!(1), request)
                .await;
            assert_eq!(response["error"]["code"], "InvalidParams");
        }
        assert!(!home.path().join(devo_core::AUTH_CONFIG_FILE_NAME).exists());
        assert!(
            !home
                .path()
                .join(devo_core::PROVIDER_CONFIG_FILE_NAME)
                .exists()
        );
    }
}

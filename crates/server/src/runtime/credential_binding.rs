//! Credential bindings and explicitly requested OAuth transport initialization.
//! These are retained authentication adapters, never a provider/model catalog.

use std::path::Path;

use devo_core::{ProviderConfigEntry, ProviderWireApi};

#[derive(Clone)]
pub(super) struct LoginTransport {
    endpoint: String,
    protocol: ProviderWireApi,
}

pub(super) fn oauth_transport_defaults(provider_id: &str) -> Option<LoginTransport> {
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
    Some(LoginTransport {
        endpoint: endpoint.to_string(),
        protocol,
    })
}

/// Pin the catalog transport only during an explicit API-key login. Ordinary
/// credential sync never adopts remote endpoints. Existing complete user routes
/// remain authoritative; partial routes require an explicit configuration repair.
pub(super) fn api_key_login_transport(
    user_dir: &Path,
    provider_id: &str,
) -> Result<Option<LoginTransport>, String> {
    let mut config = devo_core::read_provider_catalog_config(
        &user_dir.join(devo_core::PROVIDER_CONFIG_FILE_NAME),
    )
    .map_err(|error| error.to_string())?;
    let custom = devo_core::read_provider_catalog_config(
        &user_dir.join(devo_core::CUSTOM_PROVIDER_CONFIG_FILE_NAME),
    )
    .map_err(|error| error.to_string())?;
    config.merge_overlay(custom);
    if let Some(entry) = config.providers.get(provider_id) {
        let endpoint = entry
            .base_url
            .as_deref()
            .filter(|url| !url.trim().is_empty());
        match (endpoint, entry.wire_api) {
            (Some(_), Some(_)) => return Ok(None),
            _ if !entry.models.is_empty()
                && entry.models.values().all(|model| {
                    model
                        .base_url
                        .as_deref()
                        .or(endpoint)
                        .is_some_and(|url| !url.trim().is_empty())
                        && model.wire_api.or(entry.wire_api).is_some()
                }) =>
            {
                return Ok(None);
            }
            (None, None)
                if entry
                    .models
                    .values()
                    .all(|model| model.base_url.is_none() && model.wire_api.is_none()) => {}
            _ => {
                return Err(format!(
                    "provider `{provider_id}` has incomplete transport; configure its endpoint and protocol explicitly"
                ));
            }
        }
    }
    let directory = devo_core::load_remote_catalog_overlay(user_dir)
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    let entry = directory.providers.get(provider_id)
        .ok_or_else(|| format!("provider `{provider_id}` is not in the live catalog; configure its endpoint and protocol explicitly"))?;
    let endpoint = entry
        .base_url
        .as_deref()
        .filter(|url| !url.trim().is_empty())
        .ok_or_else(|| format!("provider `{provider_id}` has no catalog endpoint"))?;
    let protocol = entry
        .wire_api
        .ok_or_else(|| format!("provider `{provider_id}` has no supported catalog protocol"))?;
    Ok(Some(LoginTransport {
        endpoint: endpoint.to_string(),
        protocol,
    }))
}

/// Patch the owning file in place. Only an explicit connection request
/// supplies transport defaults; synchronization and credential refresh do not.
pub(super) fn bind_provider_credential_id(
    user_dir: &Path,
    provider_id: &str,
    credential_id: &str,
    transport: Option<LoginTransport>,
) -> Result<(), String> {
    let config_file = user_dir.join(devo_core::PROVIDER_CONFIG_FILE_NAME);
    let custom_file = user_dir.join(devo_core::CUSTOM_PROVIDER_CONFIG_FILE_NAME);
    let mut config =
        devo_core::read_provider_catalog_config(&config_file).map_err(|error| error.to_string())?;
    if let Some(entry) = config.providers.get_mut(provider_id) {
        apply_binding(entry, credential_id, transport);
        return devo_core::write_provider_catalog_config(&config_file, &config)
            .map_err(|error| error.to_string());
    }
    let mut custom =
        devo_core::read_provider_catalog_config(&custom_file).map_err(|error| error.to_string())?;
    if let Some(entry) = custom.providers.get_mut(provider_id) {
        apply_binding(entry, credential_id, transport);
        return devo_core::write_provider_catalog_config(&custom_file, &custom)
            .map_err(|error| error.to_string());
    }
    let mut entry = ProviderConfigEntry {
        name: Some(provider_id.to_string()),
        ..Default::default()
    };
    apply_binding(&mut entry, credential_id, transport);
    config.providers.insert(provider_id.to_string(), entry);
    devo_core::write_provider_catalog_config(&config_file, &config)
        .map_err(|error| error.to_string())
}

fn apply_binding(
    entry: &mut ProviderConfigEntry,
    credential_id: &str,
    transport: Option<LoginTransport>,
) {
    entry.credential = Some(credential_id.to_string());
    entry.enabled.get_or_insert(true);
    if let Some(transport) = transport {
        if entry
            .base_url
            .as_deref()
            .is_none_or(|endpoint| endpoint.trim().is_empty())
        {
            entry.base_url = Some(transport.endpoint.to_string());
        }
        entry.wire_api.get_or_insert(transport.protocol);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn explicit_oauth_connection_sets_only_missing_transport() {
        let home = tempfile::tempdir().unwrap();
        bind_provider_credential_id(
            home.path(),
            "openai-codex",
            "token",
            oauth_transport_defaults("openai-codex"),
        )
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
        bind_provider_credential_id(
            home.path(),
            "openai-codex",
            "token",
            /*transport*/ None,
        )
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
        bind_provider_credential_id(
            home.path(),
            "anthropic",
            "new-token",
            oauth_transport_defaults("anthropic"),
        )
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
    async fn configure_connection_rejects_missing_catalog_and_unknown_adapters_before_writing() {
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

#[cfg(test)]
mod api_key_tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[tokio::test]
    async fn explicit_catalog_api_key_login_pins_transport_without_persisting_directory() {
        let home = tempfile::tempdir().unwrap();
        let source = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            source.path(),
            json!({"deepseek": {
                "npm": "@ai-sdk/deepseek", "api": "https://api.deepseek.com",
                "models": {"deepseek-chat": {"tool_call": true, "limit": {"context": 64000}}}
            }})
            .to_string(),
        )
        .unwrap();
        let catalog = devo_core::CatalogConfig {
            source: source.path().to_string_lossy().into_owned(),
            ..Default::default()
        };
        assert!(matches!(
            devo_core::refresh_remote_catalog(home.path(), &catalog).await,
            devo_core::CatalogRefreshOutcome::Updated { .. }
        ));
        let runtime = crate::test_support::TestRuntime::noop().runtime(home.path());
        // Startup synchronization creates a credential-only overlay. It must not
        // adopt the remote endpoint until the user explicitly connects.
        let mut request = json!({"provider": "deepseek", "secret": "fixture", "kind": "api_key"});
        let synced = runtime
            .handle_native_credential_set(json!(1), request.clone())
            .await;
        assert!(synced.get("error").is_none(), "{synced}");
        let path = home.path().join(devo_core::PROVIDER_CONFIG_FILE_NAME);
        let sparse = devo_core::read_provider_catalog_config(&path).unwrap();
        assert_eq!(
            (
                sparse.providers["deepseek"].base_url.clone(),
                sparse.providers["deepseek"].wire_api
            ),
            (None, None)
        );
        request["configureConnection"] = json!(true);
        let response = runtime
            .handle_native_credential_set(json!(2), request)
            .await;
        assert!(response.get("error").is_none(), "{response}");
        let stored = devo_core::read_provider_catalog_config(&path).unwrap();
        let expected = serde_json::from_value(json!({"provider": {"deepseek": {
            "name": "deepseek", "credential": "deepseek", "enabled": true,
            "base_url": "https://api.deepseek.com", "wire_api": "openai_chat_completions"
        }}}))
        .unwrap();
        assert_eq!(stored, expected);
        let effective =
            devo_core::effective_provider_catalog_with_home(&stored, Some(home.path())).unwrap();
        assert_eq!(
            effective.providers["deepseek"].wire_api,
            Some(ProviderWireApi::OpenAIChatCompletions)
        );
        assert!(!effective.providers["deepseek"].models.is_empty());
        // Subsequent logins preserve the approved route even if metadata changes.
        std::fs::write(
            source.path(),
            json!({"deepseek": {
                "npm": "@ai-sdk/anthropic", "api": "https://changed.example",
                "models": {"deepseek-chat": {"tool_call": true}}
            }})
            .to_string(),
        )
        .unwrap();
        devo_core::refresh_remote_catalog_force(home.path(), &catalog).await;
        assert!(
            api_key_login_transport(home.path(), "deepseek")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn api_key_login_preserves_complete_model_scoped_transport() {
        let home = tempfile::tempdir().unwrap();
        let config: devo_core::ProviderConfigFile = serde_json::from_value(json!({"provider": {"custom": {
            "models": {"model": {"base_url": "https://user.example", "wire_api": "anthropic_messages"}}
        }}})).unwrap();
        let path = home.path().join(devo_core::PROVIDER_CONFIG_FILE_NAME);
        devo_core::write_provider_catalog_config(&path, &config).unwrap();
        assert!(
            api_key_login_transport(home.path(), "custom")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            devo_core::read_provider_catalog_config(&path).unwrap(),
            config
        );
    }

    #[test]
    fn api_key_login_preserves_mixed_inherited_model_transports() {
        for provider in [
            json!({"wire_api": "openai_responses", "models": {"model": {"base_url": "https://user.example"}}}),
            json!({"base_url": "https://user.example", "models": {"model": {"wire_api": "openai_responses"}}}),
        ] {
            let home = tempfile::tempdir().unwrap();
            let config: devo_core::ProviderConfigFile =
                serde_json::from_value(json!({"provider": {"custom": provider}})).unwrap();
            let path = home
                .path()
                .join(devo_core::CUSTOM_PROVIDER_CONFIG_FILE_NAME);
            devo_core::write_provider_catalog_config(&path, &config).unwrap();
            assert!(
                api_key_login_transport(home.path(), "custom")
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                devo_core::read_provider_catalog_config(&path).unwrap(),
                config
            );
        }
    }

    #[test]
    fn api_key_login_rejects_one_incomplete_model_without_filling_from_catalog() {
        let home = tempfile::tempdir().unwrap();
        let config: devo_core::ProviderConfigFile =
            serde_json::from_value(json!({"provider": {"custom": {
                "wire_api": "openai_responses", "models": {
                    "complete": {"base_url": "https://user.example"},
                    "incomplete": {"context_window": 64000}
                }
            }}}))
            .unwrap();
        let path = home.path().join(devo_core::PROVIDER_CONFIG_FILE_NAME);
        devo_core::write_provider_catalog_config(&path, &config).unwrap();
        assert!(api_key_login_transport(home.path(), "custom").is_err());
        assert_eq!(
            devo_core::read_provider_catalog_config(&path).unwrap(),
            config
        );
    }

    #[test]
    fn api_key_login_preserves_complete_custom_route_and_rejects_partial_routes() {
        for (endpoint, protocol, succeeds) in [
            (
                Some("https://custom.example/v1"),
                Some(ProviderWireApi::OpenAIResponses),
                true,
            ),
            (Some("https://custom.example/v1"), None, false),
            (None, Some(ProviderWireApi::OpenAIResponses), false),
        ] {
            let home = tempfile::tempdir().unwrap();
            let config = devo_core::ProviderConfigFile {
                providers: [(
                    "deepseek".to_string(),
                    ProviderConfigEntry {
                        base_url: endpoint.map(str::to_string),
                        wire_api: protocol,
                        ..Default::default()
                    },
                )]
                .into_iter()
                .collect(),
                ..Default::default()
            };
            let path = home
                .path()
                .join(devo_core::CUSTOM_PROVIDER_CONFIG_FILE_NAME);
            devo_core::write_provider_catalog_config(&path, &config).unwrap();
            assert_eq!(
                api_key_login_transport(home.path(), "deepseek").is_ok(),
                succeeds
            );
            assert_eq!(
                devo_core::read_provider_catalog_config(&path).unwrap(),
                config
            );
        }
    }
}

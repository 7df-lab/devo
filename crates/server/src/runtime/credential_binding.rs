//! Credential bindings and explicitly requested OAuth transport initialization.
//! These are retained authentication adapters, never a provider/model catalog.

use std::path::Path;

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

/// Patch the owning file in place. Only an explicit OAuth connection request
/// supplies transport defaults; synchronization and credential refresh do not.
pub(super) fn bind_provider_credential_id(
    user_dir: &Path,
    provider_id: &str,
    credential_id: &str,
    transport: Option<OAuthTransport>,
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
    transport: Option<OAuthTransport>,
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
    async fn configure_connection_rejects_api_keys_and_unknown_adapters_before_writing() {
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

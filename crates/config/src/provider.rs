mod auth;
mod persistence;
mod resolve;
mod schema;

pub use devo_protocol::ProviderWireApi;

pub use auth::AUTH_CONFIG_FILE_NAME;
pub use auth::read_user_auth_config;
pub use auth::upsert_user_auth_api_key;
pub use persistence::CONFIG_FILE_NAME;
pub use resolve::ResolvedModelBinding;
pub use resolve::load_config;
pub use resolve::provider_request_model_map_for_binding;
pub use resolve::resolve_enabled_model_binding;
pub use resolve::resolve_model_binding;
pub use resolve::resolve_provider_settings;
pub use resolve::resolve_provider_settings_from_config_and_auth;
pub use schema::*;

pub fn provider_id_for_endpoint(provider: &ProviderWireApi, _base_url: Option<&str>) -> String {
    provider.as_str().to_string()
}

pub fn provider_name_for_endpoint(provider: &ProviderWireApi, base_url: Option<&str>) -> String {
    provider_id_for_endpoint(provider, base_url)
}

pub(crate) use persistence::non_empty_string;
pub(crate) use persistence::provider_vendor_from_config;
pub(crate) use persistence::read_provider_config;
pub(crate) use persistence::read_provider_config_document;
pub(crate) use persistence::write_atomic;
pub(crate) use persistence::write_provider_config;

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::AUTH_CONFIG_FILE_NAME;
    use super::AuthCredentialConfig;
    use super::AuthCredentialKind;
    use super::ModelBindingConfig;
    use super::ModelProviderConfig;
    use super::PreferredAuthMethod;
    use super::ProviderConfigSection;
    use super::ProviderDefaultsConfig;
    use super::ProviderWireApi;
    use super::ResolvedModelBinding;
    use super::ResolvedProviderSettings;
    use super::UserAuthConfigFile;
    use super::read_user_auth_config;
    use super::resolve_enabled_model_binding;
    use super::resolve_model_binding;
    use super::resolve_provider_settings_from_config_and_auth;
    use super::upsert_user_auth_api_key;
    use super::write_provider_config;

    #[test]
    fn resolves_new_style_provider_and_model_settings() {
        let config = toml::from_str::<ProviderConfigSection>(
            r#"
model_provider = "xxxxx"
model = "gpt-5.4"
model_auto_compact_token_limit = 970000
model_context_window = 997500
model_reasoning_effort_selection = "medium"
disable_response_storage = true
preferred_auth_method = "apikey"

[defaults]
model_binding = "gpt54-xxxxx"

[providers.xxxxx]
enabled = true
name = "xxxxx"
base_url = "https://xxxxx/v1"
credential = "xxxxx_api_key"
wire_apis = ["openai_responses"]

[model_bindings.gpt54-xxxxx]
enabled = true
model_slug = "gpt-5.4"
provider = "xxxxx"
model_name = "gpt-5.4"
invocation_method = "openai_responses"
"#,
        )
        .expect("parse config");

        let auth = UserAuthConfigFile {
            credentials: [(
                "xxxxx_api_key".to_string(),
                AuthCredentialConfig {
                    kind: AuthCredentialKind::ApiKey,
                    value: "secret-value".to_string(),
                },
            )]
            .into_iter()
            .collect(),
            ..UserAuthConfigFile::default()
        };
        let resolved = resolve_provider_settings_from_config_and_auth(&config, &auth)
            .expect("resolve provider settings");

        assert_eq!(
            resolved,
            ResolvedProviderSettings {
                provider_id: "xxxxx".to_string(),
                wire_api: ProviderWireApi::OpenAIResponses,
                model: "gpt-5.4".to_string(),
                base_url: Some("https://xxxxx/v1".to_string()),
                api_key: Some("secret-value".to_string()),
                proxy_url: None,
                headers: None,
                model_auto_compact_token_limit: Some(970000),
                model_context_window: Some(997500),
                model_reasoning_effort_selection: Some("medium".to_string()),
                disable_response_storage: true,
                preferred_auth_method: Some(PreferredAuthMethod::Apikey),
            }
        );
    }

    #[test]
    fn resolving_new_style_provider_requires_user_auth_credential() {
        let config = toml::from_str::<ProviderConfigSection>(
            r#"
[defaults]
model_binding = "gpt54-xxxxx"

[providers.xxxxx]
name = "xxxxx"
credential = "xxxxx_api_key"

[model_bindings.gpt54-xxxxx]
model_slug = "gpt-5.4"
provider = "xxxxx"
model_name = "gpt-5.4"
"#,
        )
        .expect("parse config");

        let error =
            resolve_provider_settings_from_config_and_auth(&config, &UserAuthConfigFile::default())
                .expect_err("missing credential should fail");

        assert!(error.to_string().contains("xxxxx_api_key"));
        assert!(error.to_string().contains("auth.json"));
    }

    #[test]
    fn enabled_model_binding_resolves_requested_model_slug() {
        let config = provider_config_with_bindings();

        let binding =
            resolve_enabled_model_binding(&config, Some("catalog-two")).expect("resolve binding");

        assert_eq!(binding, expected_resolved_binding("two"));
    }

    #[test]
    fn enabled_model_binding_resolves_requested_model_name() {
        let config = provider_config_with_bindings();

        let binding =
            resolve_enabled_model_binding(&config, Some("vendor/two")).expect("resolve binding");

        assert_eq!(binding, expected_resolved_binding("two"));
    }

    #[test]
    fn enabled_model_binding_resolves_requested_binding_id_before_slug() {
        let mut config = provider_config_with_bindings();
        config.model_bindings.insert(
            "catalog-two".to_string(),
            ModelBindingConfig {
                enabled: true,
                model_slug: "catalog-one".to_string(),
                provider: "direct".to_string(),
                model_name: "direct/one".to_string(),
                invocation_method: ProviderWireApi::OpenAIChatCompletions,
                ..ModelBindingConfig::default()
            },
        );

        let binding =
            resolve_enabled_model_binding(&config, Some("catalog-two")).expect("resolve binding");

        assert_eq!(
            binding,
            ResolvedModelBinding {
                binding_id: "catalog-two".to_string(),
                model_slug: "catalog-one".to_string(),
                model_name: "direct/one".to_string(),
                provider_id: "direct".to_string(),
                invocation_method: ProviderWireApi::OpenAIChatCompletions,
                default_reasoning_effort: None,
                enabled: true,
            }
        );
    }

    #[test]
    fn enabled_model_binding_uses_default_binding_without_requested_model() {
        let config = provider_config_with_bindings();

        let binding = resolve_enabled_model_binding(&config, /*requested_model*/ None)
            .expect("resolve binding");

        assert_eq!(binding, expected_resolved_binding("one"));
    }

    #[test]
    fn enabled_model_binding_skips_disabled_default_binding() {
        let mut config = provider_config_with_bindings();
        config
            .model_bindings
            .get_mut("one")
            .expect("default binding")
            .enabled = false;

        let binding = resolve_enabled_model_binding(&config, /*requested_model*/ None)
            .expect("resolve binding");

        assert_eq!(binding, expected_resolved_binding("two"));
    }

    #[test]
    fn configured_model_binding_keeps_disabled_model_for_validation() {
        let mut config = provider_config_with_bindings();
        config.model = Some("catalog-one".to_string());
        config.defaults.model_binding = None;
        config
            .model_bindings
            .get_mut("one")
            .expect("configured binding")
            .enabled = false;

        let binding =
            resolve_model_binding(&config, /*requested_model*/ None).expect("resolve binding");

        assert_eq!(
            binding,
            ResolvedModelBinding {
                enabled: false,
                ..expected_resolved_binding("one")
            }
        );
    }

    fn provider_config_with_bindings() -> ProviderConfigSection {
        ProviderConfigSection {
            defaults: ProviderDefaultsConfig {
                model_binding: Some("one".to_string()),
            },
            model_bindings: [
                (
                    "one".to_string(),
                    ModelBindingConfig {
                        enabled: true,
                        model_slug: "catalog-one".to_string(),
                        provider: "openrouter".to_string(),
                        model_name: "vendor/one".to_string(),
                        invocation_method: ProviderWireApi::OpenAIResponses,
                        ..ModelBindingConfig::default()
                    },
                ),
                (
                    "two".to_string(),
                    ModelBindingConfig {
                        enabled: true,
                        model_slug: "catalog-two".to_string(),
                        provider: "openrouter".to_string(),
                        model_name: "vendor/two".to_string(),
                        invocation_method: ProviderWireApi::OpenAIResponses,
                        ..ModelBindingConfig::default()
                    },
                ),
            ]
            .into_iter()
            .collect(),
            ..ProviderConfigSection::default()
        }
    }

    fn expected_resolved_binding(binding_id: &str) -> ResolvedModelBinding {
        let model_suffix = match binding_id {
            "one" => "one",
            "two" => "two",
            _ => panic!("unexpected binding id"),
        };
        ResolvedModelBinding {
            binding_id: binding_id.to_string(),
            model_slug: format!("catalog-{model_suffix}"),
            model_name: format!("vendor/{model_suffix}"),
            provider_id: "openrouter".to_string(),
            invocation_method: ProviderWireApi::OpenAIResponses,
            default_reasoning_effort: None,
            enabled: true,
        }
    }

    #[test]
    fn user_auth_api_key_round_trips_through_auth_json() {
        let dir = tempfile::tempdir().expect("temp dir");

        upsert_user_auth_api_key(dir.path(), "openrouter_api_key", "sk-or-test")
            .expect("write credential");
        let auth =
            read_user_auth_config(&dir.path().join(AUTH_CONFIG_FILE_NAME)).expect("load auth");

        assert_eq!(
            auth,
            UserAuthConfigFile {
                credentials: [(
                    "openrouter_api_key".to_string(),
                    AuthCredentialConfig {
                        kind: AuthCredentialKind::ApiKey,
                        value: "sk-or-test".to_string(),
                    },
                )]
                .into_iter()
                .collect(),
                ..UserAuthConfigFile::default()
            }
        );
    }

    #[test]
    fn legacy_model_providers_do_not_resolve_provider_settings() {
        let config = toml::from_str::<ProviderConfigSection>(
            r#"
model_provider = "api.example.com"
model = "qwen3-coder-next"

[model_providers."api.example.com"]
name = "api.example.com"
base_url = "https://api.example.com"
api_key = "profile-key"
last_model = "qwen3-coder-next"
"#,
        )
        .expect("parse config");

        let error =
            resolve_provider_settings_from_config_and_auth(&config, &UserAuthConfigFile::default())
                .expect_err("legacy-only provider settings should not resolve");

        assert_eq!(
            error.to_string(),
            "No provider configured. Run `devo onboard` to complete setup."
        );
    }

    #[test]
    fn write_provider_config_preserves_unrelated_toml() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config_file = dir.path().join(super::CONFIG_FILE_NAME);
        std::fs::write(
            &config_file,
            r#"
schema_version = 1
model = "old-model"

[logging]
level = "debug"

[providers.existing]
name = "Old Name"
base_url = "https://old.example/v1"
custom_provider_key = "keep-me"

[providers.other]
name = "Other"
"#,
        )
        .expect("write initial config");

        write_provider_config(
            &config_file,
            &ProviderConfigSection {
                model: Some("new-model".to_string()),
                providers: [(
                    "existing".to_string(),
                    ModelProviderConfig {
                        name: "New Name".to_string(),
                        base_url: Some("https://new.example/v1".to_string()),
                        wire_apis: vec![ProviderWireApi::OpenAIResponses],
                        ..ModelProviderConfig::default()
                    },
                )]
                .into_iter()
                .collect(),
                ..ProviderConfigSection::default()
            },
        )
        .expect("write provider config");

        let written = std::fs::read_to_string(&config_file).expect("read written config");
        let document: toml::Value = toml::from_str(&written).expect("parse written config");

        assert_eq!(document["schema_version"].as_integer(), Some(1));
        assert_eq!(document["logging"]["level"].as_str(), Some("debug"));
        assert_eq!(document["model"].as_str(), Some("new-model"));
        assert_eq!(
            document["providers"]["existing"]["name"].as_str(),
            Some("New Name")
        );
        assert_eq!(
            document["providers"]["existing"]["base_url"].as_str(),
            Some("https://new.example/v1")
        );
        assert_eq!(
            document["providers"]["existing"]["custom_provider_key"].as_str(),
            Some("keep-me")
        );
        assert_eq!(
            document["providers"]["other"]["name"].as_str(),
            Some("Other")
        );
    }

    #[test]
    fn write_provider_config_removes_cleared_known_fields() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config_file = dir.path().join(super::CONFIG_FILE_NAME);
        std::fs::write(
            &config_file,
            r#"
[defaults]
model_binding = "old-binding"

[providers.existing]
name = "Old Name"
credential = "old-key"
base_url = "https://old.example/v1"

[model_bindings.existing-binding]
model_slug = "old-model"
provider = "existing"
model_name = "old-model"
invocation_method = "openai_chat_completions"
"#,
        )
        .expect("write initial config");

        write_provider_config(
            &config_file,
            &ProviderConfigSection {
                defaults: ProviderDefaultsConfig {
                    model_binding: Some("existing-binding".to_string()),
                },
                providers: [(
                    "existing".to_string(),
                    ModelProviderConfig {
                        name: "New Name".to_string(),
                        ..ModelProviderConfig::default()
                    },
                )]
                .into_iter()
                .collect(),
                model_bindings: [(
                    "existing-binding".to_string(),
                    ModelBindingConfig {
                        model_slug: "new-model".to_string(),
                        provider: "existing".to_string(),
                        model_name: "new-provider-model".to_string(),
                        invocation_method: ProviderWireApi::OpenAIResponses,
                        ..ModelBindingConfig::default()
                    },
                )]
                .into_iter()
                .collect(),
                ..ProviderConfigSection::default()
            },
        )
        .expect("write provider config");

        let written = std::fs::read_to_string(&config_file).expect("read written config");
        let document: toml::Value = toml::from_str(&written).expect("parse written config");

        assert_eq!(
            document["defaults"]["model_binding"].as_str(),
            Some("existing-binding")
        );
        assert!(
            document["providers"]["existing"]
                .get("credential")
                .is_none()
        );
        assert!(document["providers"]["existing"].get("base_url").is_none());
        assert_eq!(
            document["model_bindings"]["existing-binding"]["model_name"].as_str(),
            Some("new-provider-model")
        );
    }
}

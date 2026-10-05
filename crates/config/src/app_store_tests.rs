use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use std::fs;
use std::time::SystemTime;

use super::APP_CONFIG_FILE_NAME;
use super::AppConfigStore;
use crate::PROVIDER_CONFIG_FILE_NAME;
use crate::read_user_auth_config;
use devo_protocol::ProviderInfo;
use devo_protocol::ProviderModelInfo;
use devo_protocol::ProviderWireApi;

#[test]
fn loader_reads_standalone_provider_catalog_and_projects_provider_model() {
    let root = unique_temp_dir("provider-json-load");
    let home = root.join("home").join(".devo");
    fs::create_dir_all(&home).expect("create config dir");
    fs::write(
        home.join(PROVIDER_CONFIG_FILE_NAME),
        r#"
{
  "model": "local/qwen3",
  "provider": {
    "local": {
      "name": "Local Gateway",
      "base_url": "http://127.0.0.1:8000/v1",
      "credential": "local_key",
      "wire_api": "openai_chat_completions",
      "models": {
        "qwen3": {
          "name": "Qwen 3",
          "context_window": 131072
        }
      }
    }
  }
}
"#,
    )
    .expect("write provider config");

    let store = AppConfigStore::load(home, None).expect("load config");
    let config = store.effective_config();
    assert_eq!(config.provider.model.as_deref(), Some("local/qwen3"));
    assert_eq!(
        config.provider.defaults.model_binding.as_deref(),
        Some("local/qwen3")
    );
    assert_eq!(
        config.provider.model_bindings["local/qwen3"].request_model,
        "qwen3"
    );
    assert_eq!(
        config.provider.providers["local"].base_url.as_deref(),
        Some("http://127.0.0.1:8000/v1")
    );
    assert_eq!(
        config.provider_catalog.providers["local"].models["qwen3"]
            .name
            .as_deref(),
        Some("Qwen 3")
    );

    let _ = fs::remove_dir_all(root);
}

fn unique_temp_dir(label: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("system time after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "devo-config-{label}-{}-{nonce}",
        std::process::id()
    ))
}

#[test]
fn model_config_option_writes_model_default_projection() {
    let root = unique_temp_dir("model-default");
    let home = root.join(".devo");
    fs::create_dir_all(&home).expect("create config dir");
    fs::write(
        home.join(APP_CONFIG_FILE_NAME),
        r#"
model_provider = "openai"
model = "test-model"

[defaults]
model_binding = "test-binding"

[providers.openai]
name = "OpenAI"
base_url = "https://api.openai.com/v1"
credential = "openai_api_key"
wire_apis = ["openai_chat_completions"]
enabled = true

[model_bindings.test-binding]
model_slug = "test-model"
provider = "openai"
request_model = "test-model"
invocation_method = "openai_chat_completions"
enabled = true

[model_bindings.alt-binding]
model_slug = "alt-model"
provider = "openai"
request_model = "alt-model"
invocation_method = "openai_chat_completions"
enabled = true
"#,
    )
    .expect("write config");

    let mut store = AppConfigStore::load(home.clone(), None).expect("load config");
    store
        .set_model_config_option("model", "openai/alt-model")
        .expect("write model default");

    let config_text =
        fs::read_to_string(home.join(APP_CONFIG_FILE_NAME)).expect("read config.toml");
    assert!(
        config_text.contains("model") && config_text.contains("openai/alt-model"),
        "expected model default in config.toml, got:\n{config_text}"
    );
    let providers_text =
        fs::read_to_string(home.join(PROVIDER_CONFIG_FILE_NAME)).expect("read providers.json");
    let providers: serde_json::Value =
        serde_json::from_str(&providers_text).expect("parse providers");
    assert!(
        providers.get("model").is_none(),
        "session default must not live in providers.json"
    );
    assert_eq!(
        store
            .effective_config()
            .provider
            .defaults
            .model_binding
            .as_deref()
            .or(store.effective_config().provider.model.as_deref()),
        Some("openai/alt-model")
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn model_config_option_writes_reasoning_effort_selection() {
    let root = unique_temp_dir("reasoning-default");
    let home = root.join(".devo");
    fs::create_dir_all(&home).expect("create config dir");
    fs::write(
        home.join(APP_CONFIG_FILE_NAME),
        r#"
model_reasoning_effort_selection = "medium"
"#,
    )
    .expect("write config");

    let mut store = AppConfigStore::load(home.clone(), None).expect("load config");
    store
        .set_model_config_option("thought_level", "high")
        .expect("write reasoning default");

    let config_text =
        fs::read_to_string(home.join(APP_CONFIG_FILE_NAME)).expect("read config.toml");
    assert!(
        config_text.contains("model_reasoning_effort_selection") && config_text.contains("high"),
        "expected effort default in config.toml, got:\n{config_text}"
    );
    if home.join(PROVIDER_CONFIG_FILE_NAME).exists() {
        let providers_text =
            fs::read_to_string(home.join(PROVIDER_CONFIG_FILE_NAME)).expect("read providers.json");
        let providers: serde_json::Value =
            serde_json::from_str(&providers_text).expect("parse providers");
        assert!(providers.get("reasoning_effort").is_none());
    }
    assert_eq!(
        store
            .effective_config()
            .provider
            .model_reasoning_effort_selection
            .as_deref(),
        Some("high")
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn startup_migrates_legacy_provider_config_to_catalog_and_auth() {
    let root = unique_temp_dir("provider-startup-migration");
    let home = root.join(".devo");
    fs::create_dir_all(&home).expect("create config dir");
    fs::write(
        home.join(APP_CONFIG_FILE_NAME),
        r#"
theme = "aurora"
model_provider = "zhipu"
model = "glm-5.3"

[model_providers.zhipu]
name = "Zhipu"
base_url = "https://open.bigmodel.cn/api"
api_key = "zhipu-secret"
wire_api = "anthropic_messages"

[[model_providers.zhipu.models]]
model = "glm-5.3"

[[model_providers.zhipu.models]]
model = "glm-5.3-flash"
"#,
    )
    .expect("write legacy config");

    let store = AppConfigStore::load(home.clone(), None).expect("migrate config on startup");
    let provider_file = home
        .join(PROVIDER_CONFIG_FILE_NAME)
        .to_string_lossy()
        .into_owned();
    let provider_document: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&provider_file).expect("read provider catalog"))
            .expect("parse provider catalog");
    assert!(
        provider_document.get("model").is_none(),
        "session default must not live in providers.json"
    );
    let config_toml =
        fs::read_to_string(home.join(APP_CONFIG_FILE_NAME)).expect("read config.toml");
    assert!(
        config_toml.contains("zhipu/glm-5.3"),
        "expected model default in config.toml, got:\n{config_toml}"
    );
    assert_eq!(
        provider_document["provider"]["zhipu"]["base_url"].as_str(),
        Some("https://open.bigmodel.cn/api")
    );
    assert_eq!(
        provider_document["provider"]["zhipu"]["credential"].as_str(),
        Some("zhipu")
    );
    assert_eq!(
        provider_document["provider"]["zhipu"]["models"]["glm-5.3"]
            .get("wire_api")
            .and_then(serde_json::Value::as_str),
        Some("anthropic_messages")
    );
    assert_eq!(
        store.effective_config().provider.model.as_deref(),
        Some("zhipu/glm-5.3")
    );

    let auth_document: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(home.join("auth.json")).expect("read auth config"),
    )
    .expect("parse auth config");
    assert_eq!(auth_document["zhipu"]["key"].as_str(), Some("zhipu-secret"));

    let legacy_document: toml::Value = toml::from_str(
        &fs::read_to_string(home.join(APP_CONFIG_FILE_NAME)).expect("read migrated config"),
    )
    .expect("parse migrated config");
    assert_eq!(legacy_document["theme"].as_str(), Some("aurora"));
    assert!(legacy_document.get("model_provider").is_none());
    assert!(legacy_document.get("model_providers").is_none());
    assert_eq!(
        legacy_document.get("model").and_then(|v| v.as_str()),
        Some("zhipu/glm-5.3")
    );

    let catalog_before_restart =
        fs::read_to_string(home.join(PROVIDER_CONFIG_FILE_NAME)).expect("read catalog");
    let auth_before_restart = fs::read_to_string(home.join("auth.json")).expect("read auth");
    AppConfigStore::load(home.clone(), None).expect("reload migrated config");
    assert_eq!(
        fs::read_to_string(home.join(PROVIDER_CONFIG_FILE_NAME)).expect("read catalog"),
        catalog_before_restart
    );
    assert_eq!(
        fs::read_to_string(home.join("auth.json")).expect("read auth"),
        auth_before_restart
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn startup_migration_keeps_existing_json_values_over_legacy_toml() {
    let root = unique_temp_dir("provider-startup-migration-overlay");
    let home = root.join(".devo");
    fs::create_dir_all(&home).expect("create config dir");
    fs::write(
        home.join(APP_CONFIG_FILE_NAME),
        r#"
model_provider = "legacy"
model = "legacy-model"

[providers.legacy]
name = "Legacy"
base_url = "https://old.example/v1"
api_key = "old-secret"
wire_apis = ["openai_chat_completions"]
"#,
    )
    .expect("write legacy config");
    fs::write(
        home.join(PROVIDER_CONFIG_FILE_NAME),
        r#"
{
  "model": "legacy/new-model",
  "provider": {
    "legacy": {
      "base_url": "https://new.example/v1",
      "models": {
        "new-model": {
          "name": "New model"
        }
      }
    }
  }
}
"#,
    )
    .expect("write existing provider catalog");

    AppConfigStore::load(home.clone(), None).expect("migrate config on startup");

    let provider_document: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(home.join(PROVIDER_CONFIG_FILE_NAME)).expect("read provider catalog"),
    )
    .expect("parse provider catalog");
    assert_eq!(
        provider_document["provider"]["legacy"]["base_url"].as_str(),
        Some("https://new.example/v1")
    );
    assert_eq!(
        provider_document["provider"]["legacy"]["models"]["new-model"]["name"].as_str(),
        Some("New model")
    );
    assert!(
        provider_document.get("model").is_none(),
        "JSON catalog must not keep session defaults"
    );
    let auth = read_user_auth_config(&home.join("auth.json")).expect("read auth");
    assert_eq!(auth.credentials["legacy"].value, "old-secret");

    let legacy_document: toml::Value = toml::from_str(
        &fs::read_to_string(home.join(APP_CONFIG_FILE_NAME)).expect("read migrated config"),
    )
    .expect("parse migrated config");
    assert!(legacy_document.get("model_provider").is_none());
    assert!(legacy_document.get("providers").is_none());
    assert_eq!(
        legacy_document.get("model").and_then(|v| v.as_str()),
        Some("legacy/new-model")
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn disconnect_provider_removes_connection_and_unshared_auth_credential() {
    let root = unique_temp_dir("provider-disconnect");
    let home = root.join(".devo");
    fs::create_dir_all(&home).expect("create config dir");
    let mut store = AppConfigStore::load(home.clone(), None).expect("load config");
    store
        .upsert_provider_connection(
            ProviderInfo {
                id: "custom-provider".to_string(),
                name: "Custom Provider".to_string(),
                description: None,
                base_url: Some("https://example.com/v1".to_string()),
                credential: None,
                headers: BTreeMap::new(),
                options: None,
                request: None,
                compat: None,
                model_overrides: BTreeMap::new(),
                wire_apis: vec![ProviderWireApi::OpenAIChatCompletions],
                models: BTreeMap::new(),
                enabled: true,
            },
            None,
            None,
            Some("secret-value".to_string()),
        )
        .expect("create provider connection");

    let auth = read_user_auth_config(&home.join("auth.json")).expect("read auth");
    assert_eq!(auth.credentials["custom-provider"].value, "secret-value");
    assert_eq!(
        store.provider_connection_ids().expect("list connections"),
        vec!["custom-provider".to_string()]
    );

    store
        .disconnect_provider("custom-provider")
        .expect("disconnect provider");

    let providers = crate::read_provider_catalog_config(&home.join("providers.json"))
        .expect("read provider catalog");
    assert!(!providers.providers.contains_key("custom-provider"));
    assert!(
        store
            .provider_connection_ids()
            .expect("list connections after disconnect")
            .is_empty()
    );
    let auth = read_user_auth_config(&home.join("auth.json")).expect("read auth after disconnect");
    assert!(!auth.credentials.contains_key("custom-provider"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn connection_models_can_be_listed_and_removed_without_affecting_the_provider() {
    let root = unique_temp_dir("provider-model-remove");
    let home = root.join(".devo");
    fs::create_dir_all(&home).expect("create config dir");
    let mut store = AppConfigStore::load(home.clone(), None).expect("load config");
    let provider = ProviderInfo {
        id: "custom-provider".to_string(),
        name: "Custom Provider".to_string(),
        base_url: Some("https://example.com/v1".to_string()),
        wire_apis: vec![ProviderWireApi::OpenAIChatCompletions],
        models: BTreeMap::from([(
            "custom-model".to_string(),
            ProviderModelInfo {
                name: Some("Custom model".to_string()),
                web_search: Some(serde_json::json!({"mode": "disabled"})),
                web_fetch: Some(serde_json::json!({"mode": "provider"})),
                ..ProviderModelInfo::default()
            },
        )]),
        enabled: true,
        ..ProviderInfo::default()
    };
    store
        .upsert_provider_connection(
            provider,
            Some("custom-provider/custom-model".to_string()),
            None,
            None,
        )
        .expect("create provider connection");

    let expected = BTreeMap::from([(
        "custom-provider".to_string(),
        BTreeMap::from([(
            "custom-model".to_string(),
            ProviderModelInfo {
                name: Some("Custom model".to_string()),
                web_search: Some(serde_json::json!({"mode": "disabled"})),
                web_fetch: Some(serde_json::json!({"mode": "provider"})),
                ..ProviderModelInfo::default()
            },
        )]),
    )]);
    assert_eq!(
        store.provider_connection_models().expect("list models"),
        expected
    );

    store
        .remove_provider_model("custom-provider", "custom-model")
        .expect("remove model");
    assert_eq!(
        store
            .provider_connection_models()
            .expect("list models after removal"),
        BTreeMap::from([("custom-provider".to_string(), BTreeMap::new())])
    );
    assert_eq!(
        store.effective_config().provider.model,
        None,
        "removing the selected model clears the default"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn custom_providers_persist_in_separate_file() {
    let root = unique_temp_dir("custom-providers-file");
    let home = root.join(".devo");
    fs::create_dir_all(&home).expect("create config dir");
    let mut store = AppConfigStore::load(home.clone(), None).expect("load config");
    store
        .upsert_provider_connection(
            ProviderInfo {
                id: "my-local".to_string(),
                name: "My Local".to_string(),
                description: None,
                base_url: Some("http://127.0.0.1:8000/v1".to_string()),
                credential: None,
                headers: BTreeMap::new(),
                options: None,
                request: None,
                compat: None,
                model_overrides: BTreeMap::new(),
                wire_apis: vec![ProviderWireApi::OpenAIChatCompletions],
                models: BTreeMap::from([(
                    "local-model".to_string(),
                    ProviderModelInfo {
                        name: Some("Local Model".to_string()),
                        ..ProviderModelInfo::default()
                    },
                )]),
                enabled: true,
            },
            Some("my-local/local-model".to_string()),
            None,
            Some("local-key".to_string()),
        )
        .expect("upsert custom provider");

    let connections = crate::read_provider_catalog_config(&home.join("providers.json"))
        .expect("read providers.json");
    assert!(
        !connections.providers.contains_key("my-local"),
        "custom providers must not live in providers.json"
    );
    let custom = crate::read_provider_catalog_config(&home.join("custom-providers.json"))
        .expect("read custom-providers.json");
    assert!(custom.providers.contains_key("my-local"));
    assert!(
        custom.providers["my-local"]
            .models
            .contains_key("local-model")
    );
    assert_eq!(
        store.provider_connection_ids().expect("list"),
        vec!["my-local".to_string()]
    );

    let _ = fs::remove_dir_all(root);
}

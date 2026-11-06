use super::*;
use pretty_assertions::assert_eq;

#[test]
fn model_transport_round_trips_and_projects_wire_alias() {
    let model = ProviderModelConfig {
        base_url: Some("https://example.com/anthropic".into()),
        request_model: Some("wire-model".into()),
        wire_api: Some(devo_protocol::ProviderWireApi::AnthropicMessages),
        ..ProviderModelConfig::default()
    };
    assert_eq!(
        provider_model_config_from_info(provider_model_info_from_config(&model)),
        model
    );
    let catalog = ProviderConfigFile {
        providers: BTreeMap::from([(
            "custom".into(),
            ProviderConfigEntry {
                models: BTreeMap::from([("alias".into(), model)]),
                ..ProviderConfigEntry::default()
            },
        )]),
        ..ProviderConfigFile::default()
    };
    assert_eq!(
        catalog.to_provider_config_section().model_bindings["custom/alias"].request_model,
        "wire-model"
    );
}

#[test]
fn transport_overlay_preserves_authored_metadata() {
    let mut model = ProviderModelConfig {
        name: Some("My model".into()),
        context_window: Some(12345),
        ..ProviderModelConfig::default()
    };
    model.apply_overlay(ProviderModelConfig {
        base_url: Some("https://example.com/v1".into()),
        request_model: Some("wire-id".into()),
        ..ProviderModelConfig::default()
    });
    assert_eq!(
        model,
        ProviderModelConfig {
            name: Some("My model".into()),
            context_window: Some(12345),
            base_url: Some("https://example.com/v1".into()),
            request_model: Some("wire-id".into()),
            ..ProviderModelConfig::default()
        }
    );
}

#[test]
fn remote_connection_persists_only_transport_and_preserves_existing_overrides() {
    let root = std::env::temp_dir().join(format!(
        "devo-live-connection-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let original = ProviderConfigFile {
        providers: BTreeMap::from([(
            "custom".into(),
            ProviderConfigEntry {
                base_url: Some("https://pinned.example/v1".into()),
                credential: Some("saved-credential".into()),
                model_overrides: BTreeMap::from([(
                    "remote".into(),
                    ProviderModelConfig {
                        context_window: Some(12345),
                        ..ProviderModelConfig::default()
                    },
                )]),
                ..ProviderConfigEntry::default()
            },
        )]),
        ..ProviderConfigFile::default()
    };
    write_provider_catalog_config(&root.join("providers.json"), &original).unwrap();
    let mut store = AppConfigStore::load(root.clone(), None).unwrap();
    store
        .upsert_provider_connection(
            ProviderInfo {
                id: "custom".into(),
                name: "custom".into(),
                base_url: Some("https://pinned.example/v1".into()),
                wire_apis: vec![devo_protocol::ProviderWireApi::OpenAIChatCompletions],
                models: BTreeMap::from([(
                    "remote".into(),
                    ProviderModelInfo {
                        origin: Some(ProviderModelOrigin::Remote),
                        context_window: Some(999999),
                        cost: Some(serde_json::json!({"input": 10})),
                        request_model: Some("wire-id".into()),
                        ..ProviderModelInfo::default()
                    },
                )]),
                enabled: true,
                ..ProviderInfo::default()
            },
            None,
            None,
            None,
        )
        .unwrap();
    let saved = read_provider_catalog_config(&root.join("providers.json")).unwrap();
    assert_eq!(
        saved.providers["custom"].models["remote"],
        ProviderModelConfig {
            origin: Some(ProviderModelOrigin::Remote),
            request_model: Some("wire-id".into()),
            wire_api: Some(devo_protocol::ProviderWireApi::OpenAIChatCompletions),
            ..ProviderModelConfig::default()
        }
    );
    assert_eq!(
        saved.providers["custom"].credential,
        original.providers["custom"].credential
    );
    assert_eq!(
        saved.providers["custom"].model_overrides,
        original.providers["custom"].model_overrides
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pinning_legacy_connection_requires_explicit_existing_endpoint() {
    let root = std::env::temp_dir().join(format!(
        "devo-unpinned-connection-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let config = ProviderConfigFile {
        providers: BTreeMap::from([(
            "legacy".into(),
            ProviderConfigEntry {
                credential: Some("saved-credential".into()),
                ..ProviderConfigEntry::default()
            },
        )]),
        ..ProviderConfigFile::default()
    };
    let path = root.join("providers.json");
    write_provider_catalog_config(&path, &config).unwrap();
    let mut store = AppConfigStore::load(root.clone(), None).unwrap();
    let error = store
        .pin_provider_model(
            "legacy",
            "model",
            ProviderModelConfig {
                base_url: Some("https://remote.example/v1".into()),
                ..ProviderModelConfig::default()
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("no pinned endpoint"));
    assert_eq!(read_provider_catalog_config(&path).unwrap(), config);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_model_ids_resolve_offline_only_on_pinned_connections() {
    let mut catalog = ProviderConfigFile {
        providers: BTreeMap::from([(
            "custom".into(),
            ProviderConfigEntry {
                base_url: Some("https://backend.example/v1".into()),
                wire_api: Some(devo_protocol::ProviderWireApi::OpenAIChatCompletions),
                models: BTreeMap::from([("known".into(), ProviderModelConfig::default())]),
                ..Default::default()
            },
        )]),
        ..Default::default()
    };
    assert_eq!(
        catalog.resolve_model(Some("custom/vendor/model")).unwrap(),
        crate::ProviderModelSelection {
            provider_id: "custom".into(),
            model_id: "vendor/model".into(),
            variant_id: None,
            wire_api: devo_protocol::ProviderWireApi::OpenAIChatCompletions,
        }
    );
    assert!(
        catalog
            .resolve_model(Some("custom/known/unknown-variant"))
            .is_err()
    );
    catalog.providers.get_mut("custom").unwrap().base_url = None;
    assert!(catalog.resolve_model(Some("custom/vendor/model")).is_err());
}

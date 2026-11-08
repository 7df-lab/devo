use super::*;
use devo_core::AuthCredentialConfig;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;

struct NoRequests;
#[async_trait::async_trait]
impl ModelProviderSDK for NoRequests {
    async fn completion(&self, _: ModelRequest) -> anyhow::Result<ModelResponse> {
        panic!("discovery must not send requests")
    }
    async fn completion_stream(
        &self,
        _: ModelRequest,
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<StreamEvent>> + Send>>> {
        panic!("discovery must not send requests")
    }
    fn name(&self) -> &str {
        "test"
    }
}

fn snapshot() -> ChildModelCatalog {
    let configured: ProviderConfigFile = serde_json::from_value(serde_json::json!({
        "providers": {"local": {"base_url":"http://localhost:8000", "wire_api":"openai_chat_completions", "models": {
            "vendor/model": {"name":"Local model", "request_model":"actual-wire-name"}
        }}}
    })).unwrap();
    let catalog = PresetModelCatalog::load_from_provider_config(&configured).unwrap();
    let mut router = MultiProviderRouter::new(Arc::new(NoRequests));
    router.insert_route(
        ProviderRoute::connection("local", ProviderWireApi::OpenAIChatCompletions),
        Arc::new(NoRequests),
    );
    ChildModelCatalog {
        config: AppConfig::default(),
        home: PathBuf::new(),
        effective: configured.clone(),
        configured: configured.clone(),
        catalog,
        auth: UserAuthConfigFile::default(),
        router: Arc::new(router),
        route_inputs: ChildRouteInputs {
            providers: configured.clone(),
            http: ProviderHttpConfig::default(),
            auth: Some(UserAuthConfigFile::default()),
        },
    }
}

fn context(home: &Path, providers: &ProviderConfigFile) -> SessionRuntimeContext {
    std::fs::write(
        home.join("providers.json"),
        serde_json::to_vec(providers).unwrap(),
    )
    .unwrap();
    let store = AppConfigStore::load(home.to_path_buf(), None).unwrap();
    let catalog = PresetModelCatalog::load_from_provider_config(providers).unwrap();
    let mut router = MultiProviderRouter::new(Arc::new(NoRequests));
    router.insert_route(
        ProviderRoute::connection("local", ProviderWireApi::OpenAIChatCompletions),
        Arc::new(NoRequests),
    );
    SessionRuntimeContext::from_parts(
        Arc::new(NoRequests),
        Arc::new(router),
        Arc::new(ToolRegistry::new()),
        crate::empty_mcp_manager(),
        "local/vendor/model".to_string(),
        Arc::new(catalog),
        Arc::new(StdMutex::new(Box::new(FileSystemSkillCatalog::new(
            Default::default(),
        )))),
        AgentsMdConfig::default(),
        Arc::new(StdMutex::new(store)),
    )
}

#[test]
fn explicitly_pinned_models_work_without_directory_metadata() {
    let home = tempfile::tempdir().unwrap();
    let mut providers = snapshot().configured;
    providers.model = Some("local/vendor/model".to_string());
    providers.small_model = Some("local/vendor/small".to_string());
    providers.providers.get_mut("local").unwrap().models.clear();
    let context = context(home.path(), &providers);
    let entries = context.find_child_models(None, 100).unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|entry| (entry.selector.as_str(), entry.thinking_levels.clone()))
            .collect::<Vec<_>>(),
        vec![
            ("local/vendor/model", vec!["off".to_string()]),
            ("local/vendor/small", vec!["off".to_string()])
        ]
    );
    let (_, turn) = context
        .resolve_child_model(None, "local/vendor/model", None, None)
        .unwrap();
    assert_eq!(
        (turn.model_binding_id, turn.request_model),
        (
            Some("local/vendor/model".to_string()),
            "vendor/model".to_string()
        )
    );
    assert!(
        context
            .resolve_child_model(
                Some("local/vendor/unknown"),
                "local/vendor/model",
                None,
                None
            )
            .is_err()
    );
    assert!(
        context
            .resolve_child_model(None, "local/vendor/model", Some("high"), None)
            .is_err()
    );
}

#[test]
fn discovery_reloads_external_provider_disable_and_removal() {
    let home = tempfile::tempdir().unwrap();
    let mut providers = snapshot().configured;
    let context = context(home.path(), &providers);
    assert_eq!(context.find_child_models(None, 100).unwrap().len(), 1);
    providers.providers.get_mut("local").unwrap().enabled = Some(false);
    std::fs::write(
        home.path().join("providers.json"),
        serde_json::to_vec(&providers).unwrap(),
    )
    .unwrap();
    assert_eq!(context.find_child_models(None, 100).unwrap(), Vec::new());
    assert!(
        context
            .resolve_child_model(Some("local/vendor/model"), "other/model", None, None)
            .is_err()
    );
    providers.providers.clear();
    std::fs::write(
        home.path().join("providers.json"),
        serde_json::to_vec(&providers).unwrap(),
    )
    .unwrap();
    assert_eq!(context.find_child_models(None, 100).unwrap(), Vec::new());
}

#[test]
fn changed_endpoint_or_account_cannot_use_installed_adapter() {
    let mut current = snapshot();
    assert!(
        current
            .route_inputs
            .matches(&current, "local", "vendor/model")
    );
    current
        .configured
        .providers
        .get_mut("local")
        .unwrap()
        .base_url = Some("http://localhost:9000".to_string());
    assert!(
        !current
            .route_inputs
            .matches(&current, "local", "vendor/model")
    );
    current.configured = current.route_inputs.providers.clone();
    current.auth.credentials.insert(
        "local".to_string(),
        AuthCredentialConfig {
            kind: AuthCredentialKind::ApiKey,
            value: "new-account".to_string(),
            access: None,
            refresh: None,
            expires_at: None,
            account_id: None,
            enterprise_url: None,
        },
    );
    assert!(
        !current
            .route_inputs
            .matches(&current, "local", "vendor/model")
    );
}

#[test]
fn inherited_non_effort_variant_preserves_request_defaults() {
    let home = tempfile::tempdir().unwrap();
    let mut providers = snapshot().configured;
    providers
        .providers
        .get_mut("local")
        .unwrap()
        .models
        .get_mut("vendor/model")
        .unwrap()
        .variants
        .insert(
            "special".to_string(),
            devo_core::ProviderModelVariantConfig {
                request: Some(serde_json::json!({"special_mode": true})),
                ..Default::default()
            },
        );
    let model = providers
        .providers
        .get_mut("local")
        .unwrap()
        .models
        .get_mut("vendor/model")
        .unwrap();
    model.reasoning = Some(true);
    model.thinking_level_map = Some(BTreeMap::from([
        ("low".to_string(), Some("low".to_string())),
        ("high".to_string(), Some("high".to_string())),
    ]));
    let context = context(home.path(), &providers);
    let (_, turn) = context
        .resolve_child_model(None, "local/vendor/model/special", None, Some("off"))
        .unwrap();
    assert_eq!(
        (
            turn.model_binding_id,
            turn.variant,
            turn.provider_request_models.request_defaults().cloned()
        ),
        (
            Some("local/vendor/model".to_string()),
            Some("special".to_string()),
            Some(serde_json::json!({"special_mode": true}))
        )
    );
    let (_, changed_thinking) = context
        .resolve_child_model(
            None,
            "local/vendor/model/special",
            Some("high"),
            Some("low"),
        )
        .unwrap();
    assert_eq!(
        (
            changed_thinking.variant,
            changed_thinking.reasoning_effort_selection,
            changed_thinking
                .provider_request_models
                .request_defaults()
                .cloned()
        ),
        (
            Some("special".to_string()),
            Some("high".to_string()),
            Some(serde_json::json!({"special_mode": true}))
        )
    );
}

#[test]
fn missing_explicit_binding_never_falls_back_to_another_account() {
    let provider = ProviderConfigEntry {
        credential: Some("missing".to_string()),
        ..Default::default()
    };
    let auth = UserAuthConfigFile {
        credentials: BTreeMap::from([(
            "local".to_string(),
            AuthCredentialConfig {
                kind: AuthCredentialKind::ApiKey,
                value: "wrong-account".to_string(),
                access: None,
                refresh: None,
                expires_at: None,
                account_id: None,
                enterprise_url: None,
            },
        )]),
        ..Default::default()
    };
    assert!(!credentials_available(
        "local",
        &provider,
        ProviderWireApi::OpenAIChatCompletions,
        &auth
    ));
}

#[test]
fn selectors_preserve_slashes_and_reject_malformed_values() {
    assert_eq!(
        parse_selector("provider/vendor/model").unwrap(),
        ("provider", "vendor/model")
    );
    for selector in [
        "model",
        "",
        "/model",
        "provider/",
        "provider//model",
        "provider/model/",
        "provider /model",
        "provider/mo del",
        "provider/model\0",
    ] {
        assert!(parse_selector(selector).is_err(), "{selector}");
    }
}

#[test]
fn routes_must_be_configured_enabled_and_live() {
    let mut state = snapshot();
    assert_eq!(
        state.resolve("local/vendor/model").unwrap(),
        (
            "local",
            "vendor/model",
            ProviderRoute::connection("local", ProviderWireApi::OpenAIChatCompletions)
        )
    );
    assert!(state.resolve("LOCAL/vendor/model").is_err());
    assert!(state.resolve("local/unknown").is_err());
    state.configured.providers.get_mut("local").unwrap().enabled = Some(false);
    assert!(state.resolve("local/vendor/model").is_err());
    state.configured.providers.get_mut("local").unwrap().enabled = Some(true);
    state.router = Arc::new(MultiProviderRouter::new(Arc::new(NoRequests)));
    assert!(
        state.resolve("local/vendor/model").is_err(),
        "stale catalog does not imply live route"
    );
}

#[test]
fn directory_only_providers_and_disabled_models_are_excluded() {
    let mut state = snapshot();
    state.configured.providers.clear();
    assert!(state.resolve("local/vendor/model").is_err());
    state.configured = state.effective.clone();
    state
        .effective
        .providers
        .get_mut("local")
        .unwrap()
        .models
        .get_mut("vendor/model")
        .unwrap()
        .enabled = Some(false);
    assert!(state.resolve("local/vendor/model").is_err());
}

#[test]
fn credential_references_must_resolve_but_local_routes_can_be_credentialless() {
    let mut state = snapshot();
    let provider = state.effective.providers.get_mut("local").unwrap();
    assert!(credentials_available(
        "local",
        provider,
        ProviderWireApi::OpenAIChatCompletions,
        &state.auth
    ));
    assert!(!credentials_available(
        "local",
        provider,
        ProviderWireApi::AnthropicMessages,
        &state.auth
    ));
    provider.credential = Some("explicit".to_string());
    assert!(!credentials_available(
        "local",
        provider,
        ProviderWireApi::OpenAIChatCompletions,
        &state.auth
    ));
    state.auth.credentials.insert(
        "explicit".to_string(),
        AuthCredentialConfig {
            kind: AuthCredentialKind::ApiKey,
            value: "secret-never-serialized".to_string(),
            access: None,
            refresh: None,
            expires_at: None,
            account_id: None,
            enterprise_url: None,
        },
    );
    assert!(credentials_available(
        "local",
        provider,
        ProviderWireApi::OpenAIChatCompletions,
        &state.auth
    ));
    state.auth.credentials.get_mut("explicit").unwrap().value = "  ".to_string();
    assert!(!credentials_available(
        "local",
        provider,
        ProviderWireApi::OpenAIChatCompletions,
        &state.auth
    ));
}

#[test]
fn expired_and_empty_oauth_tokens_are_not_available() {
    let provider = ProviderConfigEntry {
        credential: Some("oauth".to_string()),
        ..Default::default()
    };
    let mut auth = UserAuthConfigFile {
        credentials: BTreeMap::from([(
            "oauth".to_string(),
            AuthCredentialConfig {
                kind: AuthCredentialKind::Oauth,
                value: String::new(),
                access: Some("token".to_string()),
                refresh: Some("refresh".to_string()),
                expires_at: Some(0),
                account_id: None,
                enterprise_url: None,
            },
        )]),
        ..Default::default()
    };
    assert!(!credentials_available(
        "local",
        &provider,
        ProviderWireApi::OpenAIResponses,
        &auth
    ));
    auth.credentials.get_mut("oauth").unwrap().expires_at = None;
    assert!(credentials_available(
        "local",
        &provider,
        ProviderWireApi::OpenAIResponses,
        &auth
    ));
    auth.credentials.get_mut("oauth").unwrap().access = None;
    assert!(!credentials_available(
        "local",
        &provider,
        ProviderWireApi::OpenAIResponses,
        &auth
    ));
}

#[test]
fn thinking_requires_exact_supported_levels_and_is_never_clamped() {
    let model = Model {
        thinking_level_map: Some(BTreeMap::from([
            ("minimal".to_string(), None),
            ("medium".to_string(), None),
            ("low".to_string(), Some("low".to_string())),
            ("high".to_string(), Some("high".to_string())),
        ])),
        reasoning: Some(true),
        ..Default::default()
    };
    assert_eq!(
        thinking_levels(&model),
        vec!["off".to_string(), "low".to_string(), "high".to_string()]
    );
    for level in thinking_levels(&model) {
        assert_eq!(strict_thinking(&model, Some(&level)).unwrap(), Some(level));
    }
    assert!(strict_thinking(&model, Some("xhigh")).is_err());
    assert!(strict_thinking(&model, Some(" HIGH ")).is_err());
    assert!(strict_thinking(&model, Some("HIGH")).is_err());
    assert_eq!(
        strict_thinking(&model, Some("high")).unwrap(),
        Some("high".to_string())
    );
    assert!(strict_thinking(&model, Some("medium")).is_err());
    assert!(strict_thinking(&model, Some("")).is_err());
    assert!(strict_thinking(&Model::default(), Some("high")).is_err());
    assert_eq!(
        strict_thinking(&Model::default(), Some("off")).unwrap(),
        Some("off".to_string())
    );
}

#[test]
fn derived_capability_levels_reject_unlisted_effort() {
    let model = Model {
        reasoning_capability: devo_protocol::ReasoningCapability::Levels(vec![
            devo_protocol::ReasoningLevelChoice::Effort(devo_protocol::ReasoningEffort::Low),
            devo_protocol::ReasoningLevelChoice::Effort(devo_protocol::ReasoningEffort::High),
        ]),
        ..Default::default()
    };
    assert!(strict_thinking(&model, Some("medium")).is_err());
    assert_eq!(
        strict_thinking(&model, Some("high")).unwrap(),
        Some("high".to_string())
    );
}

#[test]
fn model_specific_route_keeps_entire_model_identifier() {
    let model = ProviderModelConfig {
        base_url: Some("http://localhost:9000".to_string()),
        ..Default::default()
    };
    assert_eq!(
        model_route(
            "local",
            "vendor/model",
            &model,
            ProviderWireApi::OpenAIResponses
        ),
        ProviderRoute::connection_model("local", ProviderWireApi::OpenAIResponses, "vendor/model")
    );
}

#[test]
fn discovery_entry_serializes_only_safe_metadata() {
    let entry = ChildModelEntry {
        provider: "local".to_string(),
        id: "vendor/model".to_string(),
        name: "Local model".to_string(),
        selector: "local/vendor/model".to_string(),
        thinking_levels: vec!["off".to_string()],
    };
    assert_eq!(
        serde_json::to_value(entry).unwrap(),
        serde_json::json!({
            "provider":"local", "id":"vendor/model", "name":"Local model", "selector":"local/vendor/model", "thinking_levels":["off"]
        })
    );
}

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use futures::Stream;
use futures::stream;
use pretty_assertions::assert_eq;

use super::RoutedPromptProvider;
use super::prompt_turn_config;
use devo_core::AppConfig;
use devo_core::Model;
use devo_core::ModelBindingConfig;
use devo_core::PresetModelCatalog;
use devo_core::ProviderConfigSection;
use devo_core::ProviderDefaultsConfig;
use devo_core::ProviderVendorConfig;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::ProviderWireApi;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseMetadata;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::Usage;
use devo_provider::ModelProviderSDK;
use devo_provider::ProviderRoute;
use devo_provider::ProviderRouter;
use devo_provider::error::ProviderError;

fn model_request(model: &str) -> ModelRequest {
    ModelRequest {
        model: model.to_string(),
        system: None,
        messages: Vec::new(),
        max_tokens: 1,
        tools: None,
        hosted_tools: Vec::new(),
        sampling: Default::default(),
        request_thinking: None,
        reasoning_effort: None,
        extra_body: None,
    }
}

#[derive(Default)]
struct CapturingRouter {
    calls: Mutex<Vec<(ProviderRoute, String)>>,
}

impl CapturingRouter {
    fn calls(&self) -> Vec<(ProviderRoute, String)> {
        self.calls
            .lock()
            .expect("capturing router calls mutex should not be poisoned")
            .clone()
    }
}

#[async_trait]
impl ProviderRouter for CapturingRouter {
    async fn stream(
        &self,
        route: ProviderRoute,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = anyhow::Result<StreamEvent>> + Send>>, ProviderError>
    {
        self.calls
            .lock()
            .expect("capturing router calls mutex should not be poisoned")
            .push((route, request.model));
        Ok(Box::pin(stream::empty()))
    }

    async fn complete(
        &self,
        route: ProviderRoute,
        request: ModelRequest,
    ) -> Result<ModelResponse, ProviderError> {
        self.calls
            .lock()
            .expect("capturing router calls mutex should not be poisoned")
            .push((route, request.model));
        Ok(ModelResponse {
            id: "response".to_string(),
            content: vec![ResponseContent::Text("ok".to_string())],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
            metadata: ResponseMetadata::default(),
        })
    }

    fn name(&self) -> &str {
        "capturing-router"
    }
}

#[test]
fn prompt_turn_config_routes_requested_binding_to_provider_route() {
    let app_config = AppConfig {
        provider: ProviderConfigSection {
            defaults: ProviderDefaultsConfig {
                model_binding: Some("main".to_string()),
            },
            providers: BTreeMap::from([
                (
                    "default-provider".to_string(),
                    ProviderVendorConfig {
                        enabled: true,
                        wire_apis: vec![ProviderWireApi::OpenAIChatCompletions],
                        ..ProviderVendorConfig::default()
                    },
                ),
                (
                    "anthropic-provider".to_string(),
                    ProviderVendorConfig {
                        enabled: true,
                        wire_apis: vec![ProviderWireApi::AnthropicMessages],
                        ..ProviderVendorConfig::default()
                    },
                ),
                (
                    "other-provider".to_string(),
                    ProviderVendorConfig {
                        enabled: true,
                        wire_apis: vec![ProviderWireApi::AnthropicMessages],
                        ..ProviderVendorConfig::default()
                    },
                ),
            ]),
            model_bindings: BTreeMap::from([
                (
                    "main".to_string(),
                    ModelBindingConfig {
                        model_slug: "catalog-main".to_string(),
                        provider: "default-provider".to_string(),
                        model_name: "vendor/main".to_string(),
                        invocation_method: ProviderWireApi::OpenAIChatCompletions,
                        ..ModelBindingConfig::default()
                    },
                ),
                (
                    "alt".to_string(),
                    ModelBindingConfig {
                        model_slug: "catalog-alt".to_string(),
                        provider: "anthropic-provider".to_string(),
                        model_name: "vendor/alt".to_string(),
                        invocation_method: ProviderWireApi::AnthropicMessages,
                        ..ModelBindingConfig::default()
                    },
                ),
                (
                    "alt-thinking".to_string(),
                    ModelBindingConfig {
                        model_slug: "catalog-alt-thinking".to_string(),
                        provider: "anthropic-provider".to_string(),
                        model_name: "vendor/alt-thinking".to_string(),
                        invocation_method: ProviderWireApi::AnthropicMessages,
                        ..ModelBindingConfig::default()
                    },
                ),
                (
                    "other-thinking".to_string(),
                    ModelBindingConfig {
                        model_slug: "catalog-alt-thinking".to_string(),
                        provider: "other-provider".to_string(),
                        model_name: "other/alt-thinking".to_string(),
                        invocation_method: ProviderWireApi::AnthropicMessages,
                        ..ModelBindingConfig::default()
                    },
                ),
            ]),
            ..ProviderConfigSection::default()
        },
        ..AppConfig::default()
    };
    let model_catalog = PresetModelCatalog::new(vec![Model {
        slug: "catalog-alt".to_string(),
        provider: ProviderWireApi::AnthropicMessages,
        ..Model::default()
    }]);

    let turn_config = prompt_turn_config(&app_config, &model_catalog, Some("alt"), "catalog-main");

    assert_eq!(turn_config.model.slug, "catalog-alt");
    assert_eq!(turn_config.request_model, "vendor/alt");
    assert_eq!(turn_config.model_binding_id, Some("alt".to_string()));
    assert_eq!(
        turn_config.provider_route,
        ProviderRoute::binding("anthropic-provider", ProviderWireApi::AnthropicMessages)
    );
    assert_eq!(
        turn_config.provider_request_model("catalog-alt-thinking"),
        "vendor/alt-thinking"
    );
}

#[tokio::test]
async fn routed_prompt_provider_forwards_configured_route() {
    let route = ProviderRoute::binding("anthropic-provider", ProviderWireApi::AnthropicMessages);
    let router = Arc::new(CapturingRouter::default());
    let provider = RoutedPromptProvider::new(router.clone(), route.clone());

    provider
        .completion(model_request("vendor/alt"))
        .await
        .expect("complete request");

    assert_eq!(router.calls(), vec![(route, "vendor/alt".to_string())]);
}

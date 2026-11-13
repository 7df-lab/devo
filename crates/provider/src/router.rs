use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use devo_protocol::{ModelRequest, ProviderWireApi, StreamEvent};
use futures::Stream;

use crate::error::ProviderError;
use crate::provider::{ModelProviderSDK, UsageObserver};

/// Identifies the configured provider that should handle one model request.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ProviderRoute {
    /// Use the default provider selected during server bootstrap.
    Default,
    /// Use a provider selected through a configured Connection.
    Connection {
        provider_id: String,
        wire_api: ProviderWireApi,
        model_id: Option<String>,
    },
}

impl ProviderRoute {
    pub fn connection(provider_id: impl Into<String>, wire_api: ProviderWireApi) -> Self {
        Self::Connection {
            provider_id: provider_id.into(),
            wire_api,
            model_id: None,
        }
    }

    /// Select a model-specific endpoint on a configured connection.
    pub fn connection_model(
        provider_id: impl Into<String>,
        wire_api: ProviderWireApi,
        model_id: impl Into<String>,
    ) -> Self {
        Self::Connection {
            provider_id: provider_id.into(),
            wire_api,
            model_id: Some(model_id.into()),
        }
    }
}

/// Server-facing facade for model provider invocation.
///
/// Per L3-DES-ARCH-001, `ProviderRouter` is the trait that server uses to
/// invoke model providers. It dispatches to the appropriate `ModelProviderSDK`
/// implementation based on the model profile.
///
/// The server should depend on `ProviderRouter` rather than on individual
/// provider SDK implementations directly.
#[allow(clippy::double_must_use)]
#[async_trait]
pub trait ProviderRouter: Send + Sync {
    /// Send a streaming request to the appropriate provider.
    ///
    /// The router selects the correct provider adapter based on the model
    /// specified in the request, serializes the request, and returns a stream
    /// of normalized provider events.
    async fn stream(
        &self,
        route: ProviderRoute,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = anyhow::Result<StreamEvent>> + Send>>, ProviderError>;

    /// Send a non-streaming request to the appropriate provider.
    async fn complete(
        &self,
        route: ProviderRoute,
        request: ModelRequest,
    ) -> Result<devo_protocol::ModelResponse, ProviderError>;

    /// Complete a request while forwarding reported usage before completion.
    ///
    /// The default preserves compatibility for custom routers that only expose
    /// a final response. Routers wrapping provider SDKs must forward the observer
    /// to retain partial usage on failed or cancelled requests.
    async fn complete_with_usage_observer(
        &self,
        route: ProviderRoute,
        request: ModelRequest,
        usage_observer: UsageObserver,
    ) -> Result<devo_protocol::ModelResponse, ProviderError> {
        let response = self.complete(route, request).await?;
        if response.usage.is_reported() {
            usage_observer(&response.usage);
        }
        Ok(response)
    }

    /// Checks whether a route has a usable adapter without making a request.
    /// Custom routers must opt in; unknown routes are never assumed routable.
    fn has_available_route(&self, _route: &ProviderRoute) -> bool {
        false
    }

    /// Human-readable name of the router (e.g. "multi-provider", "openai-only").
    fn name(&self) -> &str;
}

/// A single-provider router that wraps a single `ModelProviderSDK`.
///
/// This is the simplest implementation of `ProviderRouter` for cases where
/// only one provider is configured.
pub struct SingleProviderRouter {
    provider: Arc<dyn ModelProviderSDK>,
}

impl SingleProviderRouter {
    pub fn new(provider: Arc<dyn ModelProviderSDK>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl ProviderRouter for SingleProviderRouter {
    fn has_available_route(&self, route: &ProviderRoute) -> bool {
        matches!(route, ProviderRoute::Default) && self.provider.is_available()
    }

    async fn stream(
        &self,
        _route: ProviderRoute,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = anyhow::Result<StreamEvent>> + Send>>, ProviderError>
    {
        self.provider
            .completion_stream(request)
            .await
            .map_err(normalize_provider_error)
    }

    async fn complete(
        &self,
        _route: ProviderRoute,
        request: ModelRequest,
    ) -> Result<devo_protocol::ModelResponse, ProviderError> {
        self.provider
            .completion(request)
            .await
            .map_err(normalize_provider_error)
    }

    async fn complete_with_usage_observer(
        &self,
        _route: ProviderRoute,
        request: ModelRequest,
        usage_observer: UsageObserver,
    ) -> Result<devo_protocol::ModelResponse, ProviderError> {
        self.provider
            .completion_with_usage_observer(request, usage_observer)
            .await
            .map_err(normalize_provider_error)
    }

    fn name(&self) -> &str {
        "single-provider"
    }
}

/// Route-aware provider router backed by configured provider SDK instances.
pub struct MultiProviderRouter {
    default_provider: Arc<dyn ModelProviderSDK>,
    providers: HashMap<ProviderRoute, Arc<dyn ModelProviderSDK>>,
}

impl MultiProviderRouter {
    pub fn new(default_provider: Arc<dyn ModelProviderSDK>) -> Self {
        Self {
            default_provider,
            providers: HashMap::new(),
        }
    }

    pub fn insert_route(&mut self, route: ProviderRoute, provider: Arc<dyn ModelProviderSDK>) {
        self.providers.insert(route, provider);
    }

    /// Whether a Connection route has an adapter registered. Lets live provider
    /// reloads verify that a connection actually reached the running router.
    pub fn has_route(&self, route: &ProviderRoute) -> bool {
        self.providers.contains_key(route)
    }

    fn provider_for_route(
        &self,
        route: &ProviderRoute,
    ) -> Result<&dyn ModelProviderSDK, ProviderError> {
        // The router owns providers for its lifetime, so dispatch can borrow the
        // selected adapter instead of cloning an Arc for each request.
        match route {
            ProviderRoute::Default => Ok(self.default_provider.as_ref()),
            ProviderRoute::Connection { provider_id, wire_api, .. } => self
                .providers
                .get(route)
                .map(Arc::as_ref)
                .ok_or_else(|| ProviderError::UnknownError {
                    message: format!(
                        "provider route not configured: provider `{provider_id}` with wire API `{wire_api}`"
                    ),
                    status_code: None,
                }),
        }
    }
}

#[async_trait]
impl ProviderRouter for MultiProviderRouter {
    fn has_available_route(&self, route: &ProviderRoute) -> bool {
        self.provider_for_route(route)
            .is_ok_and(|provider| provider.is_available())
    }

    async fn stream(
        &self,
        route: ProviderRoute,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = anyhow::Result<StreamEvent>> + Send>>, ProviderError>
    {
        self.provider_for_route(&route)?
            .completion_stream(request)
            .await
            .map_err(normalize_provider_error)
    }

    async fn complete(
        &self,
        route: ProviderRoute,
        request: ModelRequest,
    ) -> Result<devo_protocol::ModelResponse, ProviderError> {
        self.provider_for_route(&route)?
            .completion(request)
            .await
            .map_err(normalize_provider_error)
    }

    async fn complete_with_usage_observer(
        &self,
        route: ProviderRoute,
        request: ModelRequest,
        usage_observer: UsageObserver,
    ) -> Result<devo_protocol::ModelResponse, ProviderError> {
        self.provider_for_route(&route)?
            .completion_with_usage_observer(request, usage_observer)
            .await
            .map_err(normalize_provider_error)
    }

    fn name(&self) -> &str {
        "multi-provider"
    }
}

fn normalize_provider_error(error: anyhow::Error) -> ProviderError {
    // SDKs use anyhow for transport errors, but structured provider failures
    // must survive routing so retry policy and recovery hints remain accurate.
    if let Some(provider_error) = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<ProviderError>())
    {
        let mut normalized = provider_error.clone();
        let diagnostic = error.to_string();
        if diagnostic != provider_error.to_string() {
            let message = match &mut normalized {
                ProviderError::AuthenticationError { message, .. }
                | ProviderError::RateLimitError { message, .. }
                | ProviderError::ProviderServerError { message, .. }
                | ProviderError::ProviderTimeoutError { message, .. }
                | ProviderError::ContextLimitError { message, .. }
                | ProviderError::ModelNotFoundError { message, .. }
                | ProviderError::QuotaExceededError { message, .. }
                | ProviderError::ContentFilteredError { message, .. }
                | ProviderError::InvalidRequestError { message, .. }
                | ProviderError::StreamError { message, .. }
                | ProviderError::UnknownError { message, .. } => message,
            };
            // HTTP adapters attach status, request and response diagnostics as
            // anyhow context. Keep that context as well as the typed cause.
            *message = format!("{diagnostic}: {message}");
        }
        return normalized;
    }
    ProviderError::UnknownError {
        message: error.to_string(),
        status_code: None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use devo_protocol::{
        ModelResponse, RequestContent, RequestMessage, ResponseContent, ResponseMetadata,
        StopReason, Usage,
    };
    use futures::stream;
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn structured_errors_keep_outer_diagnostics_and_metadata() {
        let error = ProviderError::RateLimitError {
            message: "slow down".to_string(),
            retry_after_seconds: Some(7),
            provider_name: Some("test".to_string()),
        };
        let normalized = normalize_provider_error(anyhow::Error::new(error).context("HTTP 429"));
        assert_eq!(
            serde_json::to_value(normalized).unwrap(),
            serde_json::to_value(ProviderError::RateLimitError {
                message: "HTTP 429: slow down".to_string(),
                retry_after_seconds: Some(7),
                provider_name: Some("test".to_string()),
            })
            .unwrap()
        );
    }

    #[test]
    fn unstructured_errors_keep_the_existing_fallback() {
        let error = normalize_provider_error(anyhow::anyhow!("transport failed"));
        assert_eq!(
            serde_json::to_value(error).unwrap(),
            serde_json::to_value(ProviderError::UnknownError {
                message: "transport failed".to_string(),
                status_code: None,
            })
            .unwrap()
        );
    }

    #[derive(Default)]
    struct CapturingProvider {
        requests: Mutex<Vec<String>>,
        unavailable: bool,
    }

    impl CapturingProvider {
        fn requests(&self) -> Vec<String> {
            self.requests
                .lock()
                .expect("capturing provider requests mutex should not be poisoned")
                .clone()
        }
    }

    #[async_trait]
    impl ModelProviderSDK for CapturingProvider {
        fn is_available(&self) -> bool {
            !self.unavailable
        }

        async fn completion(&self, request: ModelRequest) -> anyhow::Result<ModelResponse> {
            self.requests
                .lock()
                .expect("capturing provider requests mutex should not be poisoned")
                .push(request.model);
            Ok(ModelResponse {
                id: "response".to_string(),
                content: vec![ResponseContent::Text("ok".to_string())],
                stop_reason: Some(StopReason::EndTurn),
                usage: Usage::default(),
                metadata: ResponseMetadata::default(),
            })
        }

        async fn completion_stream(
            &self,
            request: ModelRequest,
        ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<StreamEvent>> + Send>>>
        {
            self.requests
                .lock()
                .expect("capturing provider requests mutex should not be poisoned")
                .push(request.model);
            Ok(Box::pin(stream::empty()))
        }

        fn name(&self) -> &str {
            "capturing-provider"
        }
    }

    #[test]
    fn route_availability_checks_adapters_without_sending_requests() {
        let provider = Arc::new(CapturingProvider::default());
        let mut router = MultiProviderRouter::new(provider.clone());
        let route = ProviderRoute::connection("local", ProviderWireApi::OpenAIResponses);
        assert!(!router.has_available_route(&route));
        router.insert_route(route.clone(), provider.clone());
        assert!(router.has_available_route(&route));
        router.insert_route(
            route.clone(),
            Arc::new(CapturingProvider {
                unavailable: true,
                ..Default::default()
            }),
        );
        assert!(!router.has_available_route(&route));
        assert_eq!(provider.requests(), Vec::<String>::new());
    }

    fn request(model: &str) -> ModelRequest {
        ModelRequest {
            model_slug: devo_protocol::ModelProfileKey::Generic,
            model: model.to_string(),
            system: None,
            messages: vec![RequestMessage {
                role: "user".to_string(),
                content: vec![RequestContent::Text {
                    text: "hello".to_string(),
                }],
            }],
            max_tokens: 16,
            tools: None,
            hosted_tools: Vec::new(),
            sampling: Default::default(),
            request_thinking: None,
            reasoning_effort: None,
            extra_body: None,
        }
    }

    #[tokio::test]
    async fn multi_provider_router_dispatches_to_binding_route() {
        let default = Arc::new(CapturingProvider::default());
        let selected = Arc::new(CapturingProvider::default());
        let mut router = MultiProviderRouter::new(default.clone());
        router.insert_route(
            ProviderRoute::connection("openrouter", ProviderWireApi::OpenAIChatCompletions),
            selected.clone(),
        );

        router
            .complete(
                ProviderRoute::connection("openrouter", ProviderWireApi::OpenAIChatCompletions),
                request("vendor/model"),
            )
            .await
            .expect("route request");

        assert_eq!(default.requests(), Vec::<String>::new());
        assert_eq!(selected.requests(), vec!["vendor/model".to_string()]);
    }

    #[tokio::test]
    async fn multi_provider_router_reports_missing_binding_route() {
        let default = Arc::new(CapturingProvider::default());
        let router = MultiProviderRouter::new(default);

        let error = router
            .complete(
                ProviderRoute::connection("missing", ProviderWireApi::AnthropicMessages),
                request("claude"),
            )
            .await
            .expect_err("missing route should fail");

        assert_eq!(
            error.to_string(),
            "unknown provider error: provider route not configured: provider `missing` with wire API `anthropic_messages`"
        );
    }

    #[tokio::test]
    async fn single_provider_router_ignores_route_for_compatibility() {
        let provider = Arc::new(CapturingProvider::default());
        let router = SingleProviderRouter::new(provider.clone());

        router
            .complete(
                ProviderRoute::connection("other", ProviderWireApi::OpenAIResponses),
                request("any-model"),
            )
            .await
            .expect("single provider request");

        assert_eq!(provider.requests(), vec!["any-model".to_string()]);
    }
}

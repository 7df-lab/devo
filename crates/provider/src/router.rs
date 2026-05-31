use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use devo_protocol::{ModelRequest, StreamEvent};
use futures::Stream;

use crate::error::ProviderError;
use crate::provider::ModelProviderSDK;

/// Server-facing facade for model provider invocation.
///
/// Per L3-DES-ARCH-001, `ProviderRouter` is the trait that server uses to
/// invoke model providers. It dispatches to the appropriate `ModelProviderSDK`
/// implementation based on the model profile.
///
/// The server should depend on `ProviderRouter` rather than on individual
/// provider SDK implementations directly.
#[async_trait]
pub trait ProviderRouter: Send + Sync {
    /// Send a streaming request to the appropriate provider.
    ///
    /// The router selects the correct provider adapter based on the model
    /// specified in the request, serializes the request, and returns a stream
    /// of normalized provider events.
    async fn stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = anyhow::Result<StreamEvent>> + Send>>, ProviderError>;

    /// Send a non-streaming request to the appropriate provider.
    async fn complete(
        &self,
        request: ModelRequest,
    ) -> Result<devo_protocol::ModelResponse, ProviderError>;

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
    async fn stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = anyhow::Result<StreamEvent>> + Send>>, ProviderError>
    {
        self.provider
            .completion_stream(request)
            .await
            .map_err(|e| ProviderError::UnknownError {
                message: e.to_string(),
                status_code: None,
            })
    }

    async fn complete(
        &self,
        request: ModelRequest,
    ) -> Result<devo_protocol::ModelResponse, ProviderError> {
        self.provider
            .completion(request)
            .await
            .map_err(|e| ProviderError::UnknownError {
                message: e.to_string(),
                status_code: None,
            })
    }

    fn name(&self) -> &str {
        "single-provider"
    }
}

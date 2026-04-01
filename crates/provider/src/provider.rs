use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;

use crate::{ModelRequest, ModelResponse, StreamEvent};

/// A unified interface for LLM backends.
///
/// Implementations handle the specifics of each provider (Anthropic, OpenAI,
/// local models, etc.) while exposing a common streaming API.
#[async_trait]
pub trait ModelProvider: Send + Sync {
    /// Send a request and get a complete response.
    async fn complete(&self, request: ModelRequest) -> anyhow::Result<ModelResponse>;

    /// Send a request and get a stream of incremental events.
    async fn stream(
        &self,
        request: ModelRequest,
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<StreamEvent>> + Send>>>;

    /// Human-readable provider name (e.g. "anthropic", "openai").
    fn name(&self) -> &str;
}

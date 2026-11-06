use std::pin::Pin;

use anyhow::Result;
use async_trait::async_trait;
use devo_protocol::{ModelRequest, ModelResponse, StreamEvent};
use futures::{Stream, StreamExt};
use reqwest::Client;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde_json::Value;
use tracing::debug;

use crate::error::stream_error;
use crate::http::invalid_status_error;
use crate::{ModelProviderSDK, ProviderHttpOptions};

mod request;
mod response;
mod stream;
mod stream_error;
mod stream_tools;
#[cfg(test)]
mod tests;

use request::build_request;

/// OpenAI Responses API provider.
/// <https://developers.openai.com/api/reference/resources/responses>
/// This adapter keeps the new Responses wire format isolated from the legacy
/// chat-completions adapter so the transport can evolve independently.
pub struct OpenAIResponsesProvider {
    streaming_client: Client,
    base_url: String,
    api_key: Option<String>,
    http_options: ProviderHttpOptions,
}

impl OpenAIResponsesProvider {
    pub fn new(base_url: impl Into<String>) -> Self {
        let http_options = ProviderHttpOptions::default();
        Self {
            streaming_client: http_options
                .build_streaming_client()
                .unwrap_or_else(|_| Client::new()),
            base_url: base_url.into(),
            api_key: None,
            http_options,
        }
    }

    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    pub fn with_http_options(mut self, http_options: ProviderHttpOptions) -> Result<Self> {
        self.streaming_client = http_options.build_streaming_client()?;
        self.http_options = http_options;
        Ok(self)
    }

    fn endpoint(&self) -> String {
        format!("{}/responses", self.base_url.trim_end_matches('/'))
    }

    fn post_builder(
        &self,
        client: &Client,
        body: &Value,
        headers: &std::collections::BTreeMap<String, String>,
    ) -> reqwest::RequestBuilder {
        let builder = client
            .post(self.endpoint())
            .header(CONTENT_TYPE, "application/json");
        let builder = self
            .http_options
            .apply_request_headers(self.http_options.apply_custom_headers(builder), headers);
        let builder = if let Some(api_key) = &self.api_key {
            builder.header(AUTHORIZATION, format!("Bearer {api_key}"))
        } else {
            builder
        };
        builder.json(body)
    }

    fn streaming_request_builder(
        &self,
        body: &Value,
        headers: &std::collections::BTreeMap<String, String>,
    ) -> reqwest::RequestBuilder {
        self.post_builder(&self.streaming_client, body, headers)
    }
}

/// Folds a [`StreamEvent`] stream into a single [`ModelResponse`] for the
/// non-streaming [`ModelProviderSDK::completion`] surface.
async fn fold_stream_response(
    mut events: Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>,
) -> Result<ModelResponse> {
    let mut response: Option<ModelResponse> = None;
    while let Some(event) = events.next().await {
        if let StreamEvent::MessageDone { response: done } = event? {
            response = Some(done);
        }
    }
    response.ok_or_else(|| {
        anyhow::anyhow!(stream_error(
            "openai responses stream ended without a completed response",
        ))
    })
}

#[async_trait]
impl ModelProviderSDK for OpenAIResponsesProvider {
    async fn completion(&self, request: ModelRequest) -> Result<ModelResponse> {
        // Codex-style backends reject non-streaming requests outright
        // ("Stream must be set to true"), so the non-streaming surface is
        // served by folding the streaming events into a single response.
        let events = self.completion_stream(request).await?;
        fold_stream_response(events).await
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let body = build_request(&request, /*stream*/ true);
        debug!(
            provider = "openai-responses",
            api_base = %self.base_url,
            model = %request.model,
            messages = request.messages.len(),
            tools = request.tools.as_ref().map_or(0, Vec::len),
            max_tokens = request.max_tokens,
            "sending openai responses streaming request"
        );

        // The codex backend streams SSE without a Content-Type header, which
        // `reqwest_eventsource` refuses; decode the body directly instead.
        let response = self
            .streaming_request_builder(&body, &crate::request_headers(request.extra_body.as_ref()))
            .send()
            .await
            .map_err(|error| {
                stream_error(format!(
                    "openai responses stream request failed for model {}: {error}",
                    request.model
                ))
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(invalid_status_error(
                "openai-responses",
                &request.model,
                "stream",
                status,
                response,
                &body,
            )
            .await);
        }
        Ok(stream::decode_response_stream(response, request))
    }

    fn name(&self) -> &str {
        "openai-responses"
    }
}

//! Legacy MCP HTTP+SSE client transport.
//!
//! The current `rmcp` dependency exposes Streamable HTTP as a built-in client
//! transport, but not the older SSE transport. This module bridges the legacy
//! MCP SSE shape into `rmcp::transport::Transport` for ACP `type: "sse"` MCP
//! server entries.

use std::collections::VecDeque;

use futures::StreamExt as _;
use reqwest::Client;
use reqwest::Response;
use reqwest::StatusCode;
use reqwest::Url;
use reqwest::header::ACCEPT;
use reqwest::header::AUTHORIZATION;
use reqwest::header::CONTENT_TYPE;
use reqwest::header::HeaderMap;
use reqwest::header::HeaderName;
use reqwest::header::HeaderValue;
use rmcp::service::RoleClient;
use rmcp::service::RxJsonRpcMessage;
use rmcp::service::TxJsonRpcMessage;
use rmcp::transport::WorkerTransport;
use rmcp::transport::worker::Worker;
use rmcp::transport::worker::WorkerConfig;
use rmcp::transport::worker::WorkerContext;
use rmcp::transport::worker::WorkerQuitReason;
use rmcp::transport::worker::WorkerSendRequest;
use sse_stream::Sse;
use sse_stream::SseStream;

const EVENT_STREAM_MIME_TYPE: &str = "text/event-stream";
const JSON_MIME_TYPE: &str = "application/json";
const ACCEPT_HEADER_VALUE: &str = "text/event-stream";
const NON_JSON_RESPONSE_BODY_PREVIEW_BYTES: usize = 8_192;

pub(crate) type LegacySseTransport = WorkerTransport<LegacySseTransportWorker>;

#[derive(Debug, thiserror::Error)]
pub(crate) enum LegacySseTransportError {
    #[error("legacy SSE transport closed")]
    Closed,
    #[error("legacy SSE transport task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("HTTP request failed: {0}")]
    HttpRequest(#[from] reqwest::Error),
    #[error("invalid HTTP header value: {0}")]
    Header(String),
    #[error("invalid SSE URL `{url}`: {error}")]
    Url { url: String, error: String },
    #[error("legacy SSE endpoint event did not include message endpoint data")]
    MissingEndpointData,
    #[error("SSE stream error: {0}")]
    Sse(String),
    #[error("failed to decode server JSON-RPC message: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("POST returned HTTP {status}; body: {body}")]
    PostStatus { status: StatusCode, body: String },
    #[error("GET returned HTTP {status}; body: {body}")]
    GetStatus { status: StatusCode, body: String },
    #[error("GET returned unsupported content-type `{content_type}`; body: {body}")]
    GetContentType { content_type: String, body: String },
}

pub(crate) struct LegacySseTransportWorker {
    sse_url: String,
    http_client: Client,
    default_headers: HeaderMap,
    bearer_token: Option<String>,
}

impl LegacySseTransportWorker {
    pub(crate) fn transport(
        sse_url: String,
        default_headers: HeaderMap,
        bearer_token: Option<String>,
    ) -> LegacySseTransport {
        WorkerTransport::spawn(Self {
            sse_url,
            http_client: Client::new(),
            default_headers,
            bearer_token,
        })
    }

    async fn open_sse_stream(
        &self,
    ) -> Result<
        impl futures::Stream<Item = Result<Sse, sse_stream::Error>> + Send + 'static,
        LegacySseTransportError,
    > {
        let mut headers = self.default_headers.clone();
        insert_header(&mut headers, ACCEPT, ACCEPT_HEADER_VALUE)?;
        if let Some(token) = self.bearer_token.as_ref() {
            insert_header(&mut headers, AUTHORIZATION, format!("Bearer {token}"))?;
        }

        let response = self
            .http_client
            .get(&self.sse_url)
            .headers(headers)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let body = collect_body_preview(response).await;
            return Err(LegacySseTransportError::GetStatus { status, body });
        }

        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("missing-content-type")
            .to_string();
        if !content_type.starts_with(EVENT_STREAM_MIME_TYPE) {
            let body = collect_body_preview(response).await;
            return Err(LegacySseTransportError::GetContentType { content_type, body });
        }

        Ok(SseStream::from_byte_stream(
            response
                .bytes_stream()
                .map(|result| result.map_err(std::io::Error::other)),
        ))
    }

    async fn post_message(
        &self,
        endpoint: &str,
        message: TxJsonRpcMessage<RoleClient>,
    ) -> Result<(), LegacySseTransportError> {
        let body = serde_json::to_vec(&message)?;
        let mut headers = self.default_headers.clone();
        insert_header(&mut headers, CONTENT_TYPE, JSON_MIME_TYPE)?;
        if let Some(token) = self.bearer_token.as_ref() {
            insert_header(&mut headers, AUTHORIZATION, format!("Bearer {token}"))?;
        }

        let response = self
            .http_client
            .post(endpoint)
            .headers(headers)
            .body(body)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let body = collect_body_preview(response).await;
            return Err(LegacySseTransportError::PostStatus { status, body });
        }
        Ok(())
    }

    fn endpoint_from_event(&self, event: &Sse) -> Result<String, LegacySseTransportError> {
        let data = event
            .data
            .as_deref()
            .map(str::trim)
            .filter(|data| !data.is_empty())
            .ok_or(LegacySseTransportError::MissingEndpointData)?;
        let base = Url::parse(&self.sse_url).map_err(|error| LegacySseTransportError::Url {
            url: self.sse_url.clone(),
            error: error.to_string(),
        })?;
        base.join(data)
            .map(|url| url.to_string())
            .map_err(|error| LegacySseTransportError::Url {
                url: data.to_string(),
                error: error.to_string(),
            })
    }
}

impl Worker for LegacySseTransportWorker {
    type Error = LegacySseTransportError;
    type Role = RoleClient;

    fn err_closed() -> Self::Error {
        LegacySseTransportError::Closed
    }

    fn err_join(e: tokio::task::JoinError) -> Self::Error {
        LegacySseTransportError::Join(e)
    }

    fn config(&self) -> WorkerConfig {
        WorkerConfig {
            name: Some("legacy_sse_client".to_string()),
            channel_buffer_capacity: 16,
        }
    }

    async fn run(
        self,
        mut context: WorkerContext<Self>,
    ) -> Result<(), WorkerQuitReason<Self::Error>> {
        let mut stream = Box::pin(
            self.open_sse_stream()
                .await
                .map_err(WorkerQuitReason::fatal_context("opening legacy SSE stream"))?,
        );
        let mut endpoint = None::<String>;
        let mut pending = VecDeque::<WorkerSendRequest<Self>>::new();

        loop {
            tokio::select! {
                _ = context.cancellation_token.cancelled() => {
                    return Err(WorkerQuitReason::Cancelled);
                }
                maybe_request = context.from_handler_rx.recv() => {
                    let request = maybe_request.ok_or(WorkerQuitReason::HandlerTerminated)?;
                    if let Some(endpoint) = endpoint.as_deref() {
                        let result = self.post_message(endpoint, request.message).await;
                        if let Err(error) = &result {
                            tracing::debug!("legacy SSE POST failed: {error}");
                        }
                        let _ = request.responder.send(result);
                    } else {
                        pending.push_back(request);
                    }
                }
                maybe_event = stream.next() => {
                    let Some(event) = maybe_event else {
                        return Err(WorkerQuitReason::TransportClosed);
                    };
                    let event = event.map_err(|error| {
                        WorkerQuitReason::fatal(
                            LegacySseTransportError::Sse(error.to_string()),
                            "reading legacy SSE stream",
                        )
                    })?;

                    match event.event.as_deref() {
                        Some("endpoint") => {
                            endpoint = Some(self.endpoint_from_event(&event).map_err(
                                WorkerQuitReason::fatal_context("parsing legacy SSE endpoint"),
                            )?);
                            while let (Some(endpoint), Some(request)) =
                                (endpoint.as_deref(), pending.pop_front())
                            {
                                let result = self.post_message(endpoint, request.message).await;
                                if let Err(error) = &result {
                                    tracing::debug!("legacy SSE POST failed: {error}");
                                }
                                let _ = request.responder.send(result);
                            }
                        }
                        None | Some("") | Some("message") => {
                            let Some(data) = event.data else {
                                continue;
                            };
                            let message: RxJsonRpcMessage<RoleClient> =
                                serde_json::from_str(&data)
                                    .map_err(LegacySseTransportError::Decode)
                                    .map_err(WorkerQuitReason::fatal_context(
                                        "decoding legacy SSE server message",
                                    ))?;
                            context.send_to_handler(message).await?;
                        }
                        Some(_) => {}
                    }
                }
            }
        }
    }
}

fn insert_header(
    headers: &mut HeaderMap,
    name: HeaderName,
    value: impl AsRef<str>,
) -> Result<(), LegacySseTransportError> {
    let value = HeaderValue::from_str(value.as_ref())
        .map_err(|error| LegacySseTransportError::Header(error.to_string()))?;
    headers.insert(name, value);
    Ok(())
}

async fn collect_body_preview(response: Response) -> String {
    match response.bytes().await {
        Ok(body) => body_preview(body.as_ref()),
        Err(error) => format!("failed to read response body: {error}"),
    }
}

fn body_preview(body: &[u8]) -> String {
    let body_preview = String::from_utf8_lossy(body);
    let body_len = body_preview.len();
    if body_len <= NON_JSON_RESPONSE_BODY_PREVIEW_BYTES {
        return body_preview.into_owned();
    }

    let mut boundary = NON_JSON_RESPONSE_BODY_PREVIEW_BYTES;
    while !body_preview.is_char_boundary(boundary) {
        boundary = boundary.saturating_sub(1);
    }
    format!(
        "{}... (truncated {} bytes)",
        &body_preview[..boundary],
        body_len.saturating_sub(boundary)
    )
}

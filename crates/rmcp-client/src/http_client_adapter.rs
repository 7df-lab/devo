//! RMCP Streamable HTTP adapter backed by the local reqwest client.

use std::fmt::Write as _;
use std::io;
use std::sync::Arc;

use futures::StreamExt;
use futures::stream::BoxStream;
use reqwest::Client;
use reqwest::Response;
use reqwest::StatusCode;
use reqwest::header::ACCEPT;
use reqwest::header::AUTHORIZATION;
use reqwest::header::AsHeaderName;
use reqwest::header::CONTENT_TYPE;
use reqwest::header::HeaderMap;
use reqwest::header::HeaderName;
use rmcp::model::ClientJsonRpcMessage;
use rmcp::model::ServerJsonRpcMessage;
use rmcp::transport::streamable_http_client::AuthRequiredError;
use rmcp::transport::streamable_http_client::StreamableHttpClient;
use rmcp::transport::streamable_http_client::StreamableHttpError;
use rmcp::transport::streamable_http_client::StreamableHttpPostResponse;
use sse_stream::Sse;
use sse_stream::SseStream;

const EVENT_STREAM_MIME_TYPE: &str = "text/event-stream";
const JSON_MIME_TYPE: &str = "application/json";
const ACCEPT_HEADER_VALUE: &str = "text/event-stream, application/json";
const HEADER_SESSION_ID: &str = "Mcp-Session-Id";
const NON_JSON_RESPONSE_BODY_PREVIEW_BYTES: usize = 8_192;

#[derive(Clone)]
pub(crate) struct StreamableHttpClientAdapter {
    http_client: Client,
    default_headers: HeaderMap,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum StreamableHttpClientAdapterError {
    #[error("streamable HTTP session expired with 404 Not Found")]
    SessionExpired404,
    #[error(transparent)]
    HttpRequest(#[from] reqwest::Error),
    #[error("invalid HTTP header: {0}")]
    Header(String),
}

impl StreamableHttpClientAdapter {
    pub(crate) fn new(http_client: Client, default_headers: HeaderMap) -> Self {
        Self {
            http_client,
            default_headers,
        }
    }
}

impl StreamableHttpClient for StreamableHttpClientAdapter {
    type Error = StreamableHttpClientAdapterError;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_token: Option<String>,
    ) -> std::result::Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let mut headers = self.default_headers.clone();
        insert_header(
            &mut headers,
            ACCEPT,
            ACCEPT_HEADER_VALUE,
            StreamableHttpClientAdapterError::Header,
        )?;
        insert_header(
            &mut headers,
            CONTENT_TYPE,
            JSON_MIME_TYPE,
            StreamableHttpClientAdapterError::Header,
        )?;
        if let Some(auth_token) = auth_token {
            insert_header(
                &mut headers,
                AUTHORIZATION,
                format!("Bearer {auth_token}"),
                StreamableHttpClientAdapterError::Header,
            )?;
        }
        if let Some(session_id_value) = session_id.as_ref() {
            insert_header(
                &mut headers,
                HeaderName::from_static("mcp-session-id"),
                session_id_value.as_ref(),
                StreamableHttpClientAdapterError::Header,
            )?;
        }

        let body = serde_json::to_vec(&message).map_err(StreamableHttpError::Deserialize)?;
        let response = self
            .http_client
            .post(uri.as_ref())
            .headers(headers)
            .body(body)
            .send()
            .await
            .map_err(StreamableHttpClientAdapterError::HttpRequest)
            .map_err(StreamableHttpError::Client)?;
        let status = response.status();

        if status == StatusCode::NOT_FOUND && session_id.is_some() {
            return Err(StreamableHttpError::Client(
                StreamableHttpClientAdapterError::SessionExpired404,
            ));
        }
        if status == StatusCode::UNAUTHORIZED
            && let Some(header) =
                response_header_value(response.headers(), reqwest::header::WWW_AUTHENTICATE)
                    .map(str::to_string)
        {
            return Err(StreamableHttpError::AuthRequired(AuthRequiredError {
                www_authenticate_header: header,
            }));
        }
        if matches!(status, StatusCode::ACCEPTED | StatusCode::NO_CONTENT) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if !status.is_success() {
            let body = collect_body(response).await?;
            return Err(StreamableHttpError::UnexpectedServerResponse(
                format!(
                    "POST returned HTTP {status}; body: {}",
                    body_preview(body.as_ref())
                )
                .into(),
            ));
        }

        let content_type = response_header_value(response.headers(), CONTENT_TYPE);
        let session_id =
            response_header_value(response.headers(), HEADER_SESSION_ID).map(str::to_string);
        match content_type {
            Some(content_type) if content_type.starts_with(EVENT_STREAM_MIME_TYPE) => {
                let event_stream = sse_stream_from_body(response);
                Ok(StreamableHttpPostResponse::Sse(event_stream, session_id))
            }
            Some(content_type) if content_type.starts_with(JSON_MIME_TYPE) => {
                let body = collect_body(response).await?;
                let message: ServerJsonRpcMessage = serde_json::from_slice(body.as_ref())
                    .map_err(StreamableHttpError::Deserialize)?;
                Ok(StreamableHttpPostResponse::Json(message, session_id))
            }
            _ => {
                let content_type = content_type
                    .map(str::to_string)
                    .unwrap_or_else(|| "missing-content-type".into());
                let body = collect_body(response).await?;
                Err(StreamableHttpError::UnexpectedContentType(Some(format!(
                    "{content_type}; body: {}",
                    body_preview(body.as_ref())
                ))))
            }
        }
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session: Arc<str>,
        auth_token: Option<String>,
    ) -> std::result::Result<(), StreamableHttpError<Self::Error>> {
        let mut headers = self.default_headers.clone();
        if let Some(auth_token) = auth_token {
            insert_header(
                &mut headers,
                AUTHORIZATION,
                format!("Bearer {auth_token}"),
                StreamableHttpClientAdapterError::Header,
            )?;
        }
        insert_header(
            &mut headers,
            HeaderName::from_static("mcp-session-id"),
            session.as_ref(),
            StreamableHttpClientAdapterError::Header,
        )?;

        let response = self
            .http_client
            .delete(uri.as_ref())
            .headers(headers)
            .send()
            .await
            .map_err(StreamableHttpClientAdapterError::HttpRequest)
            .map_err(StreamableHttpError::Client)?;
        let status = response.status();

        if status == StatusCode::METHOD_NOT_ALLOWED {
            return Ok(());
        }
        if !status.is_success() {
            return Err(StreamableHttpError::UnexpectedServerResponse(
                format!("DELETE returned HTTP {status}").into(),
            ));
        }
        Ok(())
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        last_event_id: Option<String>,
        auth_token: Option<String>,
    ) -> std::result::Result<
        BoxStream<'static, std::result::Result<Sse, sse_stream::Error>>,
        StreamableHttpError<Self::Error>,
    > {
        let mut headers = self.default_headers.clone();
        insert_header(
            &mut headers,
            ACCEPT,
            ACCEPT_HEADER_VALUE,
            StreamableHttpClientAdapterError::Header,
        )?;
        insert_header(
            &mut headers,
            HeaderName::from_static("mcp-session-id"),
            session_id.as_ref(),
            StreamableHttpClientAdapterError::Header,
        )?;
        if let Some(last_event_id) = last_event_id {
            insert_header(
                &mut headers,
                HeaderName::from_static("last-event-id"),
                last_event_id,
                StreamableHttpClientAdapterError::Header,
            )?;
        }
        if let Some(auth_token) = auth_token {
            insert_header(
                &mut headers,
                AUTHORIZATION,
                format!("Bearer {auth_token}"),
                StreamableHttpClientAdapterError::Header,
            )?;
        }

        let response = self
            .http_client
            .get(uri.as_ref())
            .headers(headers)
            .send()
            .await
            .map_err(StreamableHttpClientAdapterError::HttpRequest)
            .map_err(StreamableHttpError::Client)?;
        let status = response.status();

        if status == StatusCode::METHOD_NOT_ALLOWED {
            return Err(StreamableHttpError::ServerDoesNotSupportSse);
        }
        if status == StatusCode::NOT_FOUND {
            return Err(StreamableHttpError::Client(
                StreamableHttpClientAdapterError::SessionExpired404,
            ));
        }
        if !status.is_success() {
            return Err(StreamableHttpError::UnexpectedServerResponse(
                format!("GET returned HTTP {status}").into(),
            ));
        }

        match response_header_value(response.headers(), CONTENT_TYPE) {
            Some(content_type) if is_streamable_http_content_type(content_type) => {}
            Some(content_type) => {
                return Err(StreamableHttpError::UnexpectedContentType(Some(
                    content_type.to_string(),
                )));
            }
            None => {
                return Err(StreamableHttpError::UnexpectedContentType(None));
            }
        }

        Ok(sse_stream_from_body(response))
    }
}

fn body_preview(body: &[u8]) -> String {
    let body_preview = String::from_utf8_lossy(body);
    let body_len = body_preview.len();
    if body_len > NON_JSON_RESPONSE_BODY_PREVIEW_BYTES {
        let mut boundary = NON_JSON_RESPONSE_BODY_PREVIEW_BYTES;
        while !body_preview.is_char_boundary(boundary) {
            boundary = boundary.saturating_sub(1);
        }
        let mut truncated = body_preview[..boundary].to_string();
        write!(
            &mut truncated,
            "... (truncated {} bytes)",
            body_len.saturating_sub(boundary)
        )
        .expect("writing to a String cannot fail");
        return truncated;
    }
    body_preview.into_owned()
}

fn insert_header<Error>(
    headers: &mut HeaderMap,
    name: HeaderName,
    value: impl AsRef<str>,
    map_error: impl FnOnce(String) -> Error,
) -> std::result::Result<(), StreamableHttpError<Error>>
where
    Error: std::error::Error + Send + Sync + 'static,
{
    let value = reqwest::header::HeaderValue::from_str(value.as_ref())
        .map_err(|error| StreamableHttpError::Client(map_error(error.to_string())))?;
    headers.insert(name, value);
    Ok(())
}

fn is_streamable_http_content_type(content_type: &str) -> bool {
    content_type
        .as_bytes()
        .starts_with(EVENT_STREAM_MIME_TYPE.as_bytes())
        || content_type
            .as_bytes()
            .starts_with(JSON_MIME_TYPE.as_bytes())
}

fn response_header_value(headers: &HeaderMap, name: impl AsHeaderName) -> Option<&str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

async fn collect_body(
    response: Response,
) -> std::result::Result<impl AsRef<[u8]>, StreamableHttpError<StreamableHttpClientAdapterError>> {
    // `reqwest::Response::bytes` already yields a shareable byte buffer. Keep it
    // in that representation so error previews and JSON parsing can borrow it
    // without copying the response body into a new Vec.
    response
        .bytes()
        .await
        .map_err(StreamableHttpClientAdapterError::HttpRequest)
        .map_err(StreamableHttpError::Client)
}

fn sse_stream_from_body(
    response: Response,
) -> BoxStream<'static, std::result::Result<Sse, sse_stream::Error>> {
    SseStream::from_byte_stream(
        response
            .bytes_stream()
            .map(|result| result.map_err(io::Error::other)),
    )
    .boxed()
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn body_preview_returns_short_utf8_body() {
        assert_eq!(body_preview(b"plain text"), "plain text");
    }

    #[test]
    fn body_preview_truncates_long_body() {
        let body = vec![b'a'; NON_JSON_RESPONSE_BODY_PREVIEW_BYTES + 3];

        let preview = body_preview(&body);

        assert_eq!(
            preview,
            format!(
                "{}... (truncated 3 bytes)",
                "a".repeat(NON_JSON_RESPONSE_BODY_PREVIEW_BYTES)
            )
        );
    }

    #[test]
    fn body_preview_truncates_on_utf8_boundary() {
        let mut body = vec![b'a'; NON_JSON_RESPONSE_BODY_PREVIEW_BYTES - 1];
        body.extend_from_slice("éx".as_bytes());

        let preview = body_preview(&body);

        assert_eq!(
            preview,
            format!(
                "{}... (truncated 3 bytes)",
                "a".repeat(NON_JSON_RESPONSE_BODY_PREVIEW_BYTES - 1)
            )
        );
    }
}

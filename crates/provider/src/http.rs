use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use reqwest::Client;
use reqwest::RequestBuilder;
use reqwest::Response;
use reqwest::StatusCode;
use reqwest::header::HeaderMap;
use reqwest::header::HeaderName;
use reqwest::header::HeaderValue;
use serde_json::Value;
use tracing::warn;

/// HTTP options shared by model-provider adapters.
#[derive(Clone, Debug, Default)]
pub struct ProviderHttpOptions {
    proxy_url: Option<String>,
    custom_headers: HeaderMap,
}

impl ProviderHttpOptions {
    /// Builds provider HTTP options from raw config fields.
    pub fn from_raw(proxy_url: Option<String>, headers: Option<String>) -> Result<Self> {
        Ok(Self {
            proxy_url: proxy_url.and_then(|value| non_empty_string(&value)),
            custom_headers: parse_custom_headers(headers)?,
        })
    }

    /// Returns the configured proxy URL, when present.
    pub fn proxy_url(&self) -> Option<&str> {
        self.proxy_url.as_deref()
    }

    pub(crate) fn build_client(&self, timeout: Option<Duration>) -> Result<Client> {
        let mut builder = Client::builder();
        if let Some(timeout) = timeout {
            builder = builder.timeout(timeout);
        }
        if let Some(proxy_url) = &self.proxy_url {
            let proxy = reqwest::Proxy::all(proxy_url)
                .with_context(|| format!("invalid provider HTTP proxy URL `{proxy_url}`"))?;
            builder = builder.proxy(proxy);
        }
        builder
            .build()
            .context("failed to build provider HTTP client")
    }

    pub(crate) fn apply_custom_headers(&self, builder: RequestBuilder) -> RequestBuilder {
        if self.custom_headers.is_empty() {
            builder
        } else {
            builder.headers(self.custom_headers.clone())
        }
    }
}

pub(crate) async fn invalid_status_error(
    provider: &'static str,
    model: &str,
    operation: &str,
    status: StatusCode,
    response: Response,
    request_body: &Value,
) -> anyhow::Error {
    let response_body = response
        .text()
        .await
        .unwrap_or_else(|error| format!("<failed to read response body: {error}>"));
    warn!(
        provider,
        model,
        operation,
        status = %status,
        http_body = %request_body,
        response_body = %response_body,
        "provider request failed"
    );
    anyhow::anyhow!(
        "{provider} {operation} error for model {model}: Invalid status code: {status}; response body: {response_body}"
    )
}

fn parse_custom_headers(headers: Option<String>) -> Result<HeaderMap> {
    let Some(headers) = headers.and_then(|value| non_empty_string(&value)) else {
        return Ok(HeaderMap::new());
    };
    let value: Value =
        serde_json::from_str(&headers).context("provider custom headers must be valid JSON")?;
    let object = value
        .as_object()
        .context("provider custom headers must be a JSON object string")?;
    let mut parsed = HeaderMap::new();
    for (name, value) in object {
        let header_name = HeaderName::from_bytes(name.as_bytes())
            .with_context(|| format!("invalid provider custom header name `{name}`"))?;
        let value = value
            .as_str()
            .with_context(|| format!("provider custom header `{name}` value must be a string"))?;
        let header_value = HeaderValue::from_str(value)
            .with_context(|| format!("invalid provider custom header `{name}` value"))?;
        parsed.insert(header_name, header_value);
    }
    Ok(parsed)
}

fn non_empty_string(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    /// Trace: L2-DES-APP-005
    /// Verifies: provider custom headers parse from a JSON object string.
    #[test]
    fn custom_headers_parse_json_object_string() {
        let options = ProviderHttpOptions::from_raw(
            None,
            Some(r#"{"X-Devo":"yes","Authorization":"custom"}"#.to_string()),
        )
        .expect("parse options");
        let request = options
            .apply_custom_headers(Client::new().get("http://example.com"))
            .build()
            .expect("build request");

        assert_eq!(
            request
                .headers()
                .get("x-devo")
                .expect("x-devo header")
                .to_str()
                .expect("header value"),
            "yes"
        );
        assert_eq!(
            request
                .headers()
                .get("authorization")
                .expect("authorization header")
                .to_str()
                .expect("header value"),
            "custom"
        );
    }

    /// Trace: L2-DES-APP-005
    /// Verifies: invalid provider custom header value errors do not print the value.
    #[test]
    fn custom_header_value_errors_do_not_print_value() {
        let error = ProviderHttpOptions::from_raw(
            None,
            Some("{\"X-Secret\":\"secret\\nvalue\"}".to_string()),
        )
        .expect_err("invalid header value");
        let message = error.to_string();

        assert_eq!(message, "invalid provider custom header `X-Secret` value");
        assert!(!message.contains("secret"));
    }
}

use anyhow::Context;
use anyhow::Result;
use devo_network_proxy::NetworkProxyConfig;
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
    network_proxy: NetworkProxyConfig,
    custom_headers: HeaderMap,
}

impl ProviderHttpOptions {
    /// Builds provider HTTP options from raw config fields.
    pub fn from_raw(proxy_url: Option<String>, headers: Option<String>) -> Result<Self> {
        Self::from_raw_with_no_proxy(proxy_url, None, headers)
    }

    /// Builds provider HTTP options from raw proxy, bypass, and header fields.
    pub fn from_raw_with_no_proxy(
        proxy_url: Option<String>,
        no_proxy: Option<String>,
        headers: Option<String>,
    ) -> Result<Self> {
        Ok(Self {
            network_proxy: NetworkProxyConfig {
                proxy_url: proxy_url.and_then(non_empty_owned_string),
                no_proxy: no_proxy.and_then(non_empty_owned_string),
            },
            custom_headers: parse_custom_headers(headers)?,
        })
    }

    /// Returns the configured proxy URL, when present.
    pub fn proxy_url(&self) -> Option<&str> {
        self.network_proxy.proxy_url.as_deref()
    }

    /// HTTP client for non-streaming requests with total request timeout.
    pub(crate) fn build_request_client(&self) -> Result<Client> {
        let builder = Client::builder()
            .connect_timeout(crate::timeout::connect_timeout())
            .timeout(crate::timeout::request_timeout());
        devo_network_proxy::apply_proxy_config(builder, &self.network_proxy)?
            .build()
            .context("failed to build provider HTTP client")
    }

    /// HTTP client for SSE streaming. Duration is bounded by per-chunk idle
    /// timeout in the stream layer, not a single wall-clock request timeout.
    pub(crate) fn build_streaming_client(&self) -> Result<Client> {
        let builder = Client::builder().connect_timeout(crate::timeout::connect_timeout());
        devo_network_proxy::apply_proxy_config(builder, &self.network_proxy)?
            .build()
            .context("failed to build provider streaming HTTP client")
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
    let Some(headers) = headers else {
        return Ok(HeaderMap::new());
    };
    let headers = headers.trim();
    if headers.is_empty() {
        return Ok(HeaderMap::new());
    }
    let value: Value =
        serde_json::from_str(headers).context("provider custom headers must be valid JSON")?;
    let object = value
        .as_object()
        .context("provider custom headers must be a JSON object string")?;
    let mut parsed = HeaderMap::with_capacity(object.len());
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

fn non_empty_owned_string(mut value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        // Trim in place: these values originate as owned environment/config
        // strings, so avoid allocating another `String` just to drop whitespace.
        let end = value.trim_end().len();
        value.truncate(end);
        let start = value.len() - value.trim_start().len();
        if start > 0 {
            value.drain(..start);
        }
        Some(value)
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

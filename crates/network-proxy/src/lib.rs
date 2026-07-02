//! Shared proxy configuration helpers for HTTP clients.
//!
//! Configured proxy URLs win over environment variables. Environment lookups
//! keep the common lowercase-before-uppercase precedence used by CLI tools.

use anyhow::Context;
use anyhow::Result;
use serde::Deserialize;
use serde::Serialize;

pub const HTTP_PROXY_ENV_KEYS: &[&str] = &["http_proxy", "HTTP_PROXY"];
pub const HTTPS_PROXY_ENV_KEYS: &[&str] = &["https_proxy", "HTTPS_PROXY"];
pub const ALL_PROXY_ENV_KEYS: &[&str] = &["all_proxy", "ALL_PROXY"];
pub const NO_PROXY_ENV_KEYS: &[&str] = &["NO_PROXY", "no_proxy"];

/// Network proxy settings forwarded to local HTTP clients.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkProxyConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_proxy: Option<String>,
}

/// Applies the configured proxy, or proxy environment variables when config is empty.
pub fn apply_proxy(
    builder: reqwest::ClientBuilder,
    config_proxy_url: Option<&str>,
) -> Result<reqwest::ClientBuilder> {
    apply_proxy_with_env(builder, config_proxy_url, |key| std::env::var(key))
}

/// Applies a full proxy config, or proxy environment variables when config is empty.
pub fn apply_proxy_config(
    builder: reqwest::ClientBuilder,
    config: &NetworkProxyConfig,
) -> Result<reqwest::ClientBuilder> {
    apply_proxy_with_env_options(
        builder,
        config.proxy_url.as_deref(),
        config.no_proxy.as_deref(),
        |key| std::env::var(key),
    )
}

/// Builds a reqwest client using the configured proxy or proxy environment variables.
pub fn build_client(config_proxy_url: Option<&str>) -> Result<reqwest::Client> {
    apply_proxy(reqwest::Client::builder(), config_proxy_url)?
        .build()
        .context("failed to build proxied HTTP client")
}

/// Builds a reqwest client using a full proxy config or proxy environment variables.
pub fn build_client_config(config: &NetworkProxyConfig) -> Result<reqwest::Client> {
    apply_proxy_config(reqwest::Client::builder(), config)?
        .build()
        .context("failed to build proxied HTTP client")
}

fn apply_proxy_with_env<F>(
    builder: reqwest::ClientBuilder,
    config_proxy_url: Option<&str>,
    env: F,
) -> Result<reqwest::ClientBuilder>
where
    F: Fn(&str) -> std::result::Result<String, std::env::VarError>,
{
    apply_proxy_with_env_options(builder, config_proxy_url, None, env)
}

fn apply_proxy_with_env_options<F>(
    builder: reqwest::ClientBuilder,
    config_proxy_url: Option<&str>,
    config_no_proxy: Option<&str>,
    env: F,
) -> Result<reqwest::ClientBuilder>
where
    F: Fn(&str) -> std::result::Result<String, std::env::VarError>,
{
    if let Some(proxy_url) = non_empty(config_proxy_url) {
        let no_proxy = non_empty(config_no_proxy).and_then(reqwest::NoProxy::from_string);
        return reqwest::Proxy::all(proxy_url)
            .with_context(|| format!("invalid proxy URL `{proxy_url}`"))
            .map(|proxy| proxy.no_proxy(no_proxy))
            .map(|proxy| builder.proxy(proxy));
    }

    let proxies = ProxyEnv::from_env(env);
    if proxies.http.is_none() && proxies.https.is_none() && proxies.all.is_none() {
        return Ok(builder);
    }
    let no_proxy = proxies
        .no_proxy
        .as_deref()
        .and_then(reqwest::NoProxy::from_string);
    let mut builder = builder;
    if let Some(proxy_url) = proxies.http.as_deref() {
        let proxy = reqwest::Proxy::http(proxy_url)
            .with_context(|| format!("invalid http_proxy URL `{proxy_url}`"))?
            .no_proxy(no_proxy.clone());
        builder = builder.proxy(proxy);
    }
    let https_proxy_url = proxies.https.as_deref().or(proxies.http.as_deref());
    if let Some(proxy_url) = https_proxy_url {
        let proxy = reqwest::Proxy::https(proxy_url)
            .with_context(|| format!("invalid https_proxy URL `{proxy_url}`"))?
            .no_proxy(no_proxy.clone());
        builder = builder.proxy(proxy);
    }
    if let Some(proxy_url) = proxies.all.as_deref() {
        let proxy = reqwest::Proxy::all(proxy_url)
            .with_context(|| format!("invalid all_proxy URL `{proxy_url}`"))?
            .no_proxy(no_proxy);
        builder = builder.proxy(proxy);
    }
    Ok(builder)
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ProxyEnv {
    all: Option<String>,
    http: Option<String>,
    https: Option<String>,
    no_proxy: Option<String>,
}

impl ProxyEnv {
    fn from_env<F>(env: F) -> Self
    where
        F: Fn(&str) -> std::result::Result<String, std::env::VarError>,
    {
        Self {
            all: first_env_value(ALL_PROXY_ENV_KEYS, &env),
            http: first_env_value(HTTP_PROXY_ENV_KEYS, &env),
            https: first_env_value(HTTPS_PROXY_ENV_KEYS, &env),
            no_proxy: first_env_value(NO_PROXY_ENV_KEYS, &env),
        }
    }
}

fn first_env_value<F>(keys: &[&str], env: &F) -> Option<String>
where
    F: Fn(&str) -> std::result::Result<String, std::env::VarError>,
{
    keys.iter()
        .find_map(|key| env(key).ok().and_then(non_empty_owned))
}

fn non_empty_owned(mut value: String) -> Option<String> {
    let start = value.len() - value.trim_start().len();
    let end = value.trim_end().len();
    if start >= end {
        None
    } else {
        // Env var values are already owned; trim them in place so leading
        // whitespace does not force another allocation on the configuration path.
        value.truncate(end);
        if start > 0 {
            value.drain(..start);
        }
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::Duration;

    use pretty_assertions::assert_eq;
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    use super::*;

    fn env_from(
        values: BTreeMap<&'static str, &'static str>,
    ) -> impl Fn(&str) -> Result<String, std::env::VarError> {
        move |key| {
            values
                .get(key)
                .map(|value| value.to_string())
                .ok_or(std::env::VarError::NotPresent)
        }
    }

    #[test]
    fn proxy_env_reads_lowercase_and_uppercase_values() {
        let env = env_from(BTreeMap::from([
            ("HTTP_PROXY", "http://proxy.example:8080"),
            ("https_proxy", "socks5h://proxy.example:1080"),
        ]));

        assert_eq!(
            ProxyEnv::from_env(env),
            ProxyEnv {
                all: None,
                http: Some("http://proxy.example:8080".to_string()),
                https: Some("socks5h://proxy.example:1080".to_string()),
                no_proxy: None,
            }
        );
    }

    #[test]
    fn proxy_env_prefers_lowercase_values() {
        let env = env_from(BTreeMap::from([
            ("http_proxy", "http://lower.example:8080"),
            ("HTTP_PROXY", "http://upper.example:8080"),
        ]));

        assert_eq!(
            ProxyEnv::from_env(env).http,
            Some("http://lower.example:8080".to_string())
        );
    }

    #[test]
    fn proxy_env_trims_values() {
        let env = env_from(BTreeMap::from([(
            "https_proxy",
            "  socks5h://proxy.example:1080  ",
        )]));

        assert_eq!(
            ProxyEnv::from_env(env).https,
            Some("socks5h://proxy.example:1080".to_string())
        );
    }

    #[test]
    fn proxy_env_trims_trailing_whitespace() {
        let env = env_from(BTreeMap::from([(
            "https_proxy",
            "socks5h://proxy.example:1080  ",
        )]));

        assert_eq!(
            ProxyEnv::from_env(env).https,
            Some("socks5h://proxy.example:1080".to_string())
        );
    }

    #[test]
    fn proxy_env_ignores_whitespace_only_values() {
        let env = env_from(BTreeMap::from([("http_proxy", "   ")]));

        assert_eq!(ProxyEnv::from_env(env).http, None);
    }

    #[test]
    fn proxy_env_reads_no_proxy_values() {
        let env = env_from(BTreeMap::from([
            ("NO_PROXY", "  127.0.0.1,localhost  "),
            ("no_proxy", "ignored.example"),
        ]));

        assert_eq!(
            ProxyEnv::from_env(env).no_proxy,
            Some("127.0.0.1,localhost".to_string())
        );
    }

    #[tokio::test]
    async fn http_proxy_env_applies_to_https_requests() {
        let target_url = spawn_response_server("proxied").await;
        let proxy_url = spawn_response_server("proxy").await;
        let env = move |key: &str| -> std::result::Result<String, std::env::VarError> {
            match key {
                "http_proxy" => Ok(proxy_url.clone()),
                _ => Err(std::env::VarError::NotPresent),
            }
        };

        let client = apply_proxy_with_env(
            reqwest::Client::builder().timeout(Duration::from_secs(5)),
            None,
            env,
        )
        .expect("proxy config")
        .build()
        .expect("client");
        let body = client
            .get(target_url)
            .send()
            .await
            .expect("proxied response")
            .text()
            .await
            .expect("response body");

        assert_eq!(body, "proxy");
    }

    #[tokio::test]
    async fn no_proxy_bypasses_matching_env_proxy() {
        let target_url = spawn_response_server("direct").await;
        let proxy_url = spawn_response_server("proxied").await;
        let env = move |key: &str| -> std::result::Result<String, std::env::VarError> {
            match key {
                "http_proxy" => Ok(proxy_url.clone()),
                "NO_PROXY" => Ok("127.0.0.1".to_string()),
                _ => Err(std::env::VarError::NotPresent),
            }
        };

        let client = apply_proxy_with_env(
            reqwest::Client::builder().timeout(Duration::from_secs(5)),
            None,
            env,
        )
        .expect("proxy config")
        .build()
        .expect("client");
        let body = client
            .get(target_url)
            .send()
            .await
            .expect("direct response")
            .text()
            .await
            .expect("response body");

        assert_eq!(body, "direct");
    }

    #[tokio::test]
    async fn no_proxy_bypasses_matching_configured_proxy() {
        let target_url = spawn_response_server("direct").await;
        let proxy_url = spawn_response_server("proxied").await;
        let config = NetworkProxyConfig {
            proxy_url: Some(proxy_url),
            no_proxy: Some("127.0.0.1".to_string()),
        };

        let client = apply_proxy_config(
            reqwest::Client::builder().timeout(Duration::from_secs(5)),
            &config,
        )
        .expect("proxy config")
        .build()
        .expect("client");
        let body = client
            .get(target_url)
            .send()
            .await
            .expect("direct response")
            .text()
            .await
            .expect("response body");

        assert_eq!(body, "direct");
    }

    async fn spawn_response_server(body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let address = listener.local_addr().expect("server address");
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept request");
            let mut buffer = [0_u8; 1024];
            let _ = stream.read(&mut buffer).await.expect("read request");
            let body_len = body.len();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {body_len}\r\nconnection: close\r\n\r\n{body}"
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write response");
        });
        format!("http://{address}")
    }
}

use anyhow::Context;
use anyhow::Result;
use serde::Deserialize;
use serde::Serialize;

pub const HTTP_PROXY_ENV_KEYS: &[&str] = &["http_proxy", "HTTP_PROXY"];
pub const HTTPS_PROXY_ENV_KEYS: &[&str] = &["https_proxy", "HTTPS_PROXY"];
pub const ALL_PROXY_ENV_KEYS: &[&str] = &["all_proxy", "ALL_PROXY"];

/// Network proxy settings forwarded to local HTTP clients.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkProxyConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,
}

/// Applies the configured proxy, or proxy environment variables when config is empty.
pub fn apply_proxy(
    builder: reqwest::ClientBuilder,
    config_proxy_url: Option<&str>,
) -> Result<reqwest::ClientBuilder> {
    apply_proxy_with_env(builder, config_proxy_url, |key| std::env::var(key))
}

/// Builds a reqwest client using the configured proxy or proxy environment variables.
pub fn build_client(config_proxy_url: Option<&str>) -> Result<reqwest::Client> {
    apply_proxy(reqwest::Client::builder(), config_proxy_url)?
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
    if let Some(proxy_url) = non_empty(config_proxy_url) {
        return reqwest::Proxy::all(proxy_url)
            .with_context(|| format!("invalid proxy URL `{proxy_url}`"))
            .map(|proxy| builder.proxy(proxy));
    }

    let proxies = ProxyEnv::from_env(env);
    let mut builder = builder;
    if let Some(proxy_url) = proxies.http.as_deref() {
        let proxy = reqwest::Proxy::http(proxy_url)
            .with_context(|| format!("invalid http_proxy URL `{proxy_url}`"))?;
        builder = builder.proxy(proxy);
    }
    if let Some(proxy_url) = proxies.https.as_deref() {
        let proxy = reqwest::Proxy::https(proxy_url)
            .with_context(|| format!("invalid https_proxy URL `{proxy_url}`"))?;
        builder = builder.proxy(proxy);
    }
    if let Some(proxy_url) = proxies.all.as_deref() {
        let proxy = reqwest::Proxy::all(proxy_url)
            .with_context(|| format!("invalid all_proxy URL `{proxy_url}`"))?;
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
        }
    }
}

fn first_env_value<F>(keys: &[&str], env: &F) -> Option<String>
where
    F: Fn(&str) -> std::result::Result<String, std::env::VarError>,
{
    keys.iter().find_map(|key| {
        env(key)
            .ok()
            .and_then(|value| non_empty(Some(&value)).map(str::to_string))
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use pretty_assertions::assert_eq;

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
}

//! Shared RMCP client utility helpers.
//!
//! Stdio server launch inherits a curated environment, while streamable HTTP
//! transports share default-header parsing. The small injectable env seams keep
//! tests deterministic without mutating process-global environment variables.

use anyhow::Result;
use anyhow::anyhow;
use devo_config::McpServerEnvVar;
use reqwest::ClientBuilder;
use reqwest::header::HeaderMap;
use reqwest::header::HeaderName;
use reqwest::header::HeaderValue;
use std::collections::HashMap;
use std::env;
use std::ffi::OsString;

pub(crate) fn create_env_for_mcp_server(
    extra_env: Option<HashMap<OsString, OsString>>,
    env_vars: &[McpServerEnvVar],
) -> Result<HashMap<OsString, OsString>> {
    create_env_for_mcp_server_with_env(extra_env, env_vars, |name| env::var_os(name))
}

fn create_env_for_mcp_server_with_env<F>(
    extra_env: Option<HashMap<OsString, OsString>>,
    env_vars: &[McpServerEnvVar],
    mut lookup_env: F,
) -> Result<HashMap<OsString, OsString>>
where
    F: FnMut(&str) -> Option<OsString>,
{
    let additional_env_vars = local_stdio_env_var_names(env_vars)?;
    let extra_env_len = extra_env.as_ref().map_or(0, HashMap::len);
    let mut env = HashMap::with_capacity(DEFAULT_ENV_VARS.len() + env_vars.len() + extra_env_len);
    for var in DEFAULT_ENV_VARS.iter().copied().chain(additional_env_vars) {
        if let Some(value) = lookup_env(var) {
            env.insert(OsString::from(var), value);
        }
    }
    if let Some(extra_env) = extra_env {
        env.extend(extra_env);
    }
    Ok(env)
}

fn local_stdio_env_var_names(env_vars: &[McpServerEnvVar]) -> Result<impl Iterator<Item = &str>> {
    if let Some(remote_var) = env_vars.iter().find(|var| var.is_remote_source()) {
        return Err(anyhow!(
            "env_vars entry `{}` uses source `remote`, but Devo does not support remote stdio MCP execution",
            remote_var.name()
        ));
    }
    Ok(env_vars.iter().map(McpServerEnvVar::name))
}

pub(crate) fn build_default_headers(
    http_headers: Option<HashMap<String, String>>,
    env_http_headers: Option<HashMap<String, String>>,
) -> Result<HeaderMap> {
    build_default_headers_with_env(http_headers, env_http_headers, |name| env::var(name).ok())
}

fn build_default_headers_with_env<F>(
    http_headers: Option<HashMap<String, String>>,
    env_http_headers: Option<HashMap<String, String>>,
    mut lookup_env: F,
) -> Result<HeaderMap>
where
    F: FnMut(&str) -> Option<String>,
{
    let header_capacity = http_headers.as_ref().map_or(0, HashMap::len)
        + env_http_headers.as_ref().map_or(0, HashMap::len);
    let mut headers = HeaderMap::with_capacity(header_capacity);

    if let Some(static_headers) = http_headers {
        for (name, value) in static_headers {
            insert_default_header(
                &mut headers,
                &name,
                value.as_str(),
                HeaderValueSource::Static,
            );
        }
    }

    if let Some(env_headers) = env_http_headers {
        for (name, env_var) in env_headers {
            if let Some(value) = lookup_env(&env_var) {
                if value.trim().is_empty() {
                    continue;
                }

                insert_default_header(
                    &mut headers,
                    &name,
                    value.as_str(),
                    HeaderValueSource::Env { env_var: &env_var },
                );
            }
        }
    }

    Ok(headers)
}

enum HeaderValueSource<'a> {
    Static,
    Env { env_var: &'a str },
}

fn insert_default_header(
    headers: &mut HeaderMap,
    name: &str,
    value: &str,
    source: HeaderValueSource<'_>,
) {
    let header_name = match HeaderName::from_bytes(name.as_bytes()) {
        Ok(name) => name,
        Err(err) => {
            tracing::warn!("invalid HTTP header name `{name}`: {err}");
            return;
        }
    };
    let header_value = match HeaderValue::from_str(value) {
        Ok(value) => value,
        Err(err) => {
            match source {
                HeaderValueSource::Static => {
                    tracing::warn!("invalid HTTP header value for `{name}`: {err}");
                }
                HeaderValueSource::Env { env_var } => {
                    tracing::warn!(
                        "invalid HTTP header value read from {env_var} for `{name}`: {err}"
                    );
                }
            }
            return;
        }
    };
    headers.insert(header_name, header_value);
}

pub(crate) fn apply_default_headers(
    builder: ClientBuilder,
    default_headers: &HeaderMap,
) -> ClientBuilder {
    if default_headers.is_empty() {
        builder
    } else {
        builder.default_headers(default_headers.clone())
    }
}

#[cfg(unix)]
pub(crate) const DEFAULT_ENV_VARS: &[&str] = &[
    "HOME",
    "LOGNAME",
    "PATH",
    "SHELL",
    "USER",
    "__CF_USER_TEXT_ENCODING",
    "LANG",
    "LC_ALL",
    "TERM",
    "TMPDIR",
    "TZ",
];

#[cfg(windows)]
pub(crate) const DEFAULT_ENV_VARS: &[&str] = &[
    "PATH",
    "PATHEXT",
    "SHELL",
    "COMSPEC",
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "USERNAME",
    "USERDOMAIN",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "PROGRAMW6432",
    "PROGRAMDATA",
    "LOCALAPPDATA",
    "APPDATA",
    "TEMP",
    "TMP",
    "TMPDIR",
    "POWERSHELL",
    "PWSH",
];

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    use std::ffi::OsStr;

    #[tokio::test]
    async fn create_env_honors_overrides() {
        let value = "custom".to_string();
        let expected = OsString::from(&value);
        let env = create_env_for_mcp_server(
            Some(HashMap::from([(OsString::from("TZ"), expected.clone())])),
            &[],
        )
        .expect("local MCP env should build");
        assert_eq!(env.get(OsStr::new("TZ")), Some(&expected));
    }

    #[test]
    fn create_env_includes_additional_whitelisted_variables() {
        let custom_var = "EXTRA_RMCP_ENV";
        let expected = OsString::from("from-env");
        let env = create_env_for_mcp_server_with_env(
            /*extra_env*/ None,
            &[custom_var.into()],
            |name| (name == custom_var).then(|| expected.clone()),
        )
        .expect("local MCP env should build");
        assert_eq!(env.get(OsStr::new(custom_var)), Some(&expected));
    }

    #[test]
    fn build_default_headers_reads_static_and_env_headers() {
        let headers = build_default_headers_with_env(
            Some(HashMap::from([(
                "X-Static".to_string(),
                "configured".to_string(),
            )])),
            Some(HashMap::from([(
                "X-Env".to_string(),
                "RMCP_HEADER_VALUE".to_string(),
            )])),
            |name| (name == "RMCP_HEADER_VALUE").then(|| "from-env".to_string()),
        )
        .expect("headers should build");

        assert_eq!(
            headers
                .get("x-static")
                .and_then(|value| value.to_str().ok()),
            Some("configured")
        );
        assert_eq!(
            headers.get("x-env").and_then(|value| value.to_str().ok()),
            Some("from-env")
        );
    }

    #[test]
    fn build_default_headers_skips_blank_env_values() {
        let headers = build_default_headers_with_env(
            /*http_headers*/ None,
            Some(HashMap::from([(
                "X-Blank".to_string(),
                "RMCP_BLANK_HEADER".to_string(),
            )])),
            |name| (name == "RMCP_BLANK_HEADER").then(|| "   ".to_string()),
        )
        .expect("headers should build");

        assert!(!headers.contains_key("x-blank"));
    }

    #[test]
    fn create_local_env_rejects_remote_source_variables() {
        let err = create_env_for_mcp_server(
            /*extra_env*/ None,
            &[McpServerEnvVar::Config {
                name: "REMOTE".to_string(),
                source: Some("remote".to_string()),
            }],
        )
        .expect_err("remote source should require remote stdio");

        assert!(
            err.to_string()
                .contains("does not support remote stdio MCP execution"),
            "unexpected error: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn create_env_preserves_path_when_it_is_not_utf8() {
        use std::os::unix::ffi::OsStrExt;

        let raw_path = std::ffi::OsStr::from_bytes(b"/tmp/devo-\xFF/bin");
        let expected = raw_path.to_os_string();

        let env = create_env_for_mcp_server_with_env(/*extra_env*/ None, &[], |name| {
            (name == "PATH").then(|| expected.clone())
        })
        .expect("local MCP env should build");

        assert_eq!(env.get(OsStr::new("PATH")), Some(&expected));
    }
}

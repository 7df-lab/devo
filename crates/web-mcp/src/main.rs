//! Private stdio MCP companion. A process connects to exactly one supplier.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use devo_core::{
    AUTH_CONFIG_FILE_NAME, AuthCredentialKind, ProviderHttpConfig, read_user_auth_config,
};
use devo_network_proxy::NetworkProxyConfig;
use devo_util_paths::{APP_CONFIG_DIR_NAME, APP_CONFIG_FILE_NAME};
use rmcp::ErrorData;
use rmcp::ServiceExt;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ListToolsResult, PaginatedRequestParams,
    ServerCapabilities, ServerInfo, Tool, ToolAnnotations,
};
use serde::Deserialize;
use serde_json::json;

mod deepseek;
mod tavily;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Supplier {
    Tavily,
    DeepSeek,
}

impl Supplier {
    fn credential_id(self) -> &'static str {
        match self {
            Self::Tavily => "tavily",
            Self::DeepSeek => "deepseek",
        }
    }
}

#[derive(Clone)]
struct WebMcpServer {
    supplier: Supplier,
    http: reqwest::Client,
    api_key: Arc<str>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    query: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FetchArgs {
    url: String,
}

impl ServerHandler for WebMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..ServerInfo::default()
        }
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: web_tools(),
            next_cursor: None,
            meta: None,
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let arguments = serde_json::Value::Object(request.arguments.unwrap_or_default());
        let bad_args = |_| ErrorData::invalid_params("tool arguments are invalid", None);
        match request.name.as_ref() {
            "web_search" => {
                let SearchArgs { query } = serde_json::from_value(arguments).map_err(bad_args)?;
                match self.supplier {
                    Supplier::Tavily => tavily::search(&self.http, &self.api_key, query).await,
                    Supplier::DeepSeek => deepseek::search(&self.http, &self.api_key, query).await,
                }
            }
            "web_fetch" => {
                let FetchArgs { url } = serde_json::from_value(arguments).map_err(bad_args)?;
                match self.supplier {
                    Supplier::Tavily => tavily::fetch(&self.http, &self.api_key, url).await,
                    Supplier::DeepSeek => deepseek::fetch(&self.http, &self.api_key, url).await,
                }
            }
            _ => Err(ErrorData::invalid_params("unknown tool", None)),
        }
    }
}

fn web_tools() -> Vec<Tool> {
    [
        ("web_search", "Search the web for the query.", "query"),
        (
            "web_fetch",
            "Fetch a public HTTP(S) URL as Markdown.",
            "url",
        ),
    ]
    .into_iter()
    .map(|(name, description, argument)| {
        let schema = json!({
            "type": "object",
            "properties": { (argument): {"type":"string"} },
            "required": [argument],
            "additionalProperties": false,
        });
        let mut tool = Tool::new(
            name,
            description,
            Arc::new(schema.as_object().expect("object schema").clone()),
        );
        tool.annotations = Some(ToolAnnotations::new().read_only(true));
        tool
    })
    .collect()
}

fn valid_fetch_url(input: &str) -> bool {
    if input.len() > 8192
        || input
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control() || ch == '\\')
    {
        return false;
    }
    // URL parsers can normalize extra slashes into a host. Require an explicit
    // HTTP(S) authority and reject even empty user-info before normalization.
    let Some((scheme, rest)) = input.split_once("://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    (scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
        && !authority.is_empty()
        && !authority.contains('@')
        && reqwest::Url::parse(input).is_ok_and(|url| url.has_host())
}

const USAGE: &str =
    "usage: devo-web-mcp <tavily|deepseek> --config-dir PATH [--workspace-root PATH]";

struct Launch {
    supplier: Supplier,
    config_dir: PathBuf,
    workspace_root: Option<PathBuf>,
}

fn parse_launch() -> Result<Launch, &'static str> {
    let mut args = std::env::args_os().skip(1);
    let supplier = match args.next().as_deref().and_then(std::ffi::OsStr::to_str) {
        Some("tavily") => Supplier::Tavily,
        Some("deepseek") => Supplier::DeepSeek,
        _ => return Err(USAGE),
    };
    if args.next().as_deref().and_then(std::ffi::OsStr::to_str) != Some("--config-dir") {
        return Err(USAGE);
    }
    let config_dir = args
        .next()
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or(USAGE)?;
    let workspace_root = match args.next().as_deref().and_then(std::ffi::OsStr::to_str) {
        None => None,
        Some("--workspace-root") => Some(
            args.next()
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .ok_or(USAGE)?,
        ),
        _ => return Err(USAGE),
    };
    if args.next().is_some() {
        return Err(USAGE);
    }
    Ok(Launch {
        supplier,
        config_dir,
        workspace_root,
    })
}

fn read_proxy(path: &Path) -> Result<ProviderHttpConfig, &'static str> {
    #[derive(Deserialize)]
    struct Config {
        #[serde(default)]
        provider_http: ProviderHttpConfig,
    }
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str::<Config>(&text)
            .map(|config| config.provider_http)
            .map_err(|_| "invalid proxy configuration"),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(ProviderHttpConfig::default()),
        Err(_) => Err("unable to read proxy configuration"),
    }
}

fn start_server(launch: Launch) -> Result<WebMcpServer, &'static str> {
    let auth = read_user_auth_config(&launch.config_dir.join(AUTH_CONFIG_FILE_NAME))
        .map_err(|_| "unable to read web MCP credentials")?;
    let id = launch.supplier.credential_id();
    let api_key = auth
        .credentials
        .get(id)
        .filter(|credential| credential.kind == AuthCredentialKind::ApiKey)
        .map(|credential| credential.value.trim())
        .filter(|key| !key.is_empty())
        .map(Arc::from)
        .ok_or("API key required for the selected web supplier")?;
    let user_proxy = read_proxy(&launch.config_dir.join(APP_CONFIG_FILE_NAME))?;
    let workspace_proxy = launch
        .workspace_root
        .as_deref()
        .map(|root| read_proxy(&root.join(APP_CONFIG_DIR_NAME).join(APP_CONFIG_FILE_NAME)))
        .transpose()?
        .unwrap_or_default();
    let proxy = NetworkProxyConfig {
        proxy_url: workspace_proxy.proxy_url.or(user_proxy.proxy_url),
        no_proxy: workspace_proxy.no_proxy.or(user_proxy.no_proxy),
    };
    let http = devo_network_proxy::apply_proxy_config(
        reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none()),
        &proxy,
    )
    .map_err(|_| "unable to initialize web MCP network client")?
    .build()
    .map_err(|_| "unable to initialize web MCP network client")?;
    Ok(WebMcpServer {
        supplier: launch.supplier,
        http,
        api_key,
    })
}

#[tokio::main]
async fn main() {
    let result = async {
        let running = start_server(parse_launch()?)?
            .serve((tokio::io::stdin(), tokio::io::stdout()))
            .await
            .map_err(|_| "unable to start web MCP server")?;
        running
            .waiting()
            .await
            .map_err(|_| "web MCP server stopped unexpectedly")
    }
    .await;
    if let Err(message) = result {
        eprintln!("{message}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn same_two_canonical_tools_for_every_supplier() {
        let tools = web_tools();
        assert_eq!(
            tools
                .iter()
                .map(|tool| (tool.name.as_ref(), &tool.input_schema["required"]))
                .collect::<Vec<_>>(),
            [
                ("web_search", &json!(["query"])),
                ("web_fetch", &json!(["url"]))
            ]
        );
    }
}

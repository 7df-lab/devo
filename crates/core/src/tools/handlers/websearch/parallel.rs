//! Anonymous search using Devo's Streamable HTTP MCP transport.

use std::time::Duration;

use devo_config::ResolvedLocalWebSearchConfig;
use devo_rmcp_client::RmcpClient;
use rmcp::model::{ClientCapabilities, Implementation, InitializeRequestParams};
use serde::Deserialize;
use serde_json::Value;

use crate::contracts::{ToolCallError, ToolContext};

const ENDPOINT: &str = "https://search.parallel.ai/mcp";
const USER_AGENT: &str = concat!("devo/", env!("CARGO_PKG_VERSION"), " Parallel Search MCP");

pub(super) async fn search(
    config: &ResolvedLocalWebSearchConfig,
    query: &str,
    max_results: u32,
    ctx: &ToolContext,
) -> Result<String, ToolCallError> {
    if max_results == 0 {
        return Err(ToolCallError::InvalidInput(
            "max_results must be positive".into(),
        ));
    }
    let timeout = Duration::from_millis(ctx.budgets.wall_time_limit_ms.unwrap_or(60_000));
    let proxy = devo_network_proxy::NetworkProxyConfig {
        proxy_url: ctx.network_proxy.clone(),
        no_proxy: ctx.network_no_proxy.clone(),
    };
    let http_client = devo_network_proxy::apply_proxy_config(
        reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(timeout),
        &proxy,
    )
    .and_then(|builder| builder.build().map_err(Into::into))
    .map_err(|error| ToolCallError::ExecutionFailed(format!("Parallel HTTP client: {error}")))?;
    let client = RmcpClient::new_anonymous_streamable_http_client(
        config.base_url.as_deref().unwrap_or(ENDPOINT),
        http_client,
    )
    .await
    .map_err(|error| ToolCallError::ExecutionFailed(format!("Parallel MCP client: {error}")))?;
    let operation = async {
        client
            .initialize(
                InitializeRequestParams {
                    meta: None,
                    capabilities: ClientCapabilities::default(),
                    client_info: Implementation {
                        name: "devo".into(),
                        version: env!("CARGO_PKG_VERSION").into(),
                        ..Implementation::default()
                    },
                    protocol_version: Default::default(),
                },
                Some(timeout),
                Box::new(|_, _| {
                    Box::pin(async {
                        anyhow::bail!("Anonymous search does not support elicitation")
                    })
                }),
            )
            .await?;
        let tools = client.list_tools(/*params*/ None, Some(timeout)).await?;
        if !tools.tools.iter().any(|tool| tool.name == "web_search") {
            anyhow::bail!("Parallel MCP server does not expose web_search");
        }
        let result = client
            .call_tool(
                "web_search".into(),
                Some(serde_json::json!({
                    "objective": query,
                    "search_queries": [query],
                    "session_id": ctx.session_id.to_string(),
                })),
                /*meta*/ None,
                Some(timeout),
            )
            .await?;
        if result.is_error == Some(true) {
            anyhow::bail!(
                "Parallel MCP web_search returned an error: {:?}",
                result.content
            );
        }
        // Parallel exposes the same structured response in structuredContent and
        // as JSON text for clients that do not consume MCP structured output.
        let value = match result.structured_content {
            Some(value) => value,
            None => result
                .content
                .iter()
                .filter_map(|content| content.as_text())
                .find_map(|content| serde_json::from_str::<Value>(&content.text).ok())
                .ok_or_else(|| {
                    anyhow::anyhow!("Parallel MCP search returned no search result JSON")
                })?,
        };
        let response: SearchResponse = serde_json::from_value(value)?;
        Ok::<_, anyhow::Error>(super::format_results(
            query,
            response
                .results
                .into_iter()
                .take(max_results as usize)
                .map(|result| super::SearchResultLine {
                    title: result.title,
                    url: Some(result.url),
                    snippet: Some(result.excerpts.join("\n")),
                }),
        ))
    };
    let result = tokio::select! {
        biased;
        () = ctx.cancel_token.cancelled() => Err(ToolCallError::Cancelled),
        result = tokio::time::timeout(timeout, operation) => match result {
            Ok(result) => result.map_err(|error| ToolCallError::ExecutionFailed(format!("Parallel search failed: {error}"))),
            Err(_) => Err(ToolCallError::ExecutionFailed("Parallel search timed out".into())),
        },
    };
    client.shutdown().await;
    result
}

#[derive(Deserialize)]
struct SearchResponse {
    results: Vec<SearchResult>,
}

#[derive(Deserialize)]
struct SearchResult {
    title: Option<String>,
    url: String,
    excerpts: Vec<String>,
}

#[cfg(test)]
mod tests;

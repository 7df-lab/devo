//! Integration coverage for Streamable HTTP RMCP over a URL endpoint.
//!
//! Remote MCP in Devo is represented by HTTP/SSE/Streamable HTTP endpoints,
//! not by remote stdio process execution.

mod streamable_http_test_support;

use pretty_assertions::assert_eq;

use streamable_http_test_support::call_echo_tool;
use streamable_http_test_support::create_client;
use streamable_http_test_support::expected_echo_result;
use streamable_http_test_support::spawn_streamable_http_server;

/// What this tests: the RMCP Streamable HTTP adapter can initialize a server
/// and call a tool through a URL endpoint using local reqwest networking.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn streamable_http_url_client_round_trips_with_reqwest() -> anyhow::Result<()> {
    let (_server, base_url) = spawn_streamable_http_server().await?;
    let client = create_client(&base_url).await?;

    let result = call_echo_tool(&client, "url").await?;
    assert_eq!(result, expected_echo_result("url"));

    Ok(())
}

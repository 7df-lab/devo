use std::sync::{Arc, Mutex};

use devo_config::{
    ResolvedWebSearchConfig, UserAuthConfigFile, WebSearchConfig, resolve_web_search_config,
};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::contracts::{ToolAgentScope, ToolBudgets, ToolContext, ToolResultContent};
use crate::invocation::ToolCallId;
use crate::tool_handler::ToolHandler;

use super::super::WebSearchHandler;
use super::USER_AGENT;

fn context() -> ToolContext {
    ToolContext {
        output_store: None,
        tool_call_id: ToolCallId("parallel-search-test".into()),
        session_id: "1234567890abcdef1234567890abcdef".into(),
        turn_id: None,
        workspace_root: std::env::temp_dir(),
        budgets: ToolBudgets {
            output_limit_bytes: 32768,
            wall_time_limit_ms: Some(5000),
        },
        cancel_token: tokio_util::sync::CancellationToken::new(),
        agent_scope: ToolAgentScope::Parent,
        collaboration_mode: devo_protocol::CollaborationMode::Build,
        agent_coordinator: None,
        client_filesystem: None,
        file_read_ledger: None,
        network_proxy: None,
        network_no_proxy: Some("127.0.0.1".into()),
        sandbox_profile: None,
        sandbox_permission_overlay: None,
        kernel: None,
        python_cell_first_wait_ms: None,
        python_cell_watch: None,
        python_cell_completion: None,
        session_dir: None,
    }
}

fn input(endpoint: &str) -> Value {
    let global: WebSearchConfig = toml::from_str(&format!(
        "mode = 'local'\nlocal_provider = 'parallel'\n[local_providers.parallel]\nkind = 'parallel'\nbase_url = '{endpoint}'\nmax_results = 1"
    )).expect("parse user configuration");
    let ResolvedWebSearchConfig::Local(config) = resolve_web_search_config(
        &global,
        /*provider_override*/ None,
        /*model_override*/ None,
        &UserAuthConfigFile::default(),
    )
    .expect("resolve with empty user auth") else {
        panic!("expected local search")
    };
    assert_eq!(config.api_key, String::new());
    json!({"query":"Rust language official website", "__devo_local_web_search":config})
}

type Requests = Arc<Mutex<Vec<(String, Value)>>>;

enum Reply {
    Search,
    TextSearch,
    ToolError,
    RateLimited,
    Slow,
}

async fn server(reply: Reply) -> (String, Requests, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fixture");
    let url = format!("http://{}/mcp", listener.local_addr().expect("address"));
    let requests: Requests = Arc::default();
    let capture = requests.clone();
    let task = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.expect("accept fixture request");
            let mut bytes = Vec::new();
            let mut chunk = [0; 4096];
            let (headers, body) = loop {
                let read = socket.read(&mut chunk).await.expect("read fixture request");
                if read == 0 {
                    break (String::new(), json!({}));
                }
                bytes.extend_from_slice(&chunk[..read]);
                if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                    let headers = String::from_utf8(bytes[..end].to_vec()).expect("headers");
                    let len = headers
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().expect("length"))
                        })
                        .unwrap_or(0);
                    if bytes.len() >= end + 4 + len {
                        let body = serde_json::from_slice(&bytes[end + 4..end + 4 + len])
                            .unwrap_or(json!({}));
                        break (headers, body);
                    }
                }
            };
            capture
                .lock()
                .expect("capture lock")
                .push((headers.clone(), body.clone()));
            let method = body["method"].as_str().unwrap_or("");
            if matches!(reply, Reply::Slow) {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            let search = json!({"results":[
                {"title":"Rust", "url":"https://www.rust-lang.org/", "excerpts":["Rust programming language."]},
                {"title":"Other", "url":"https://example.com/", "excerpts":["Truncated by max_results."]}
            ]});
            let result = match method {
                "initialize" => {
                    json!({"protocolVersion":"2025-06-18", "capabilities":{"tools":{}}, "serverInfo":{"name":"fixture", "version":"1"}})
                }
                "tools/list" => {
                    json!({"tools":[{"name":"web_search", "inputSchema":{"type":"object"}}]})
                }
                "tools/call" if matches!(reply, Reply::ToolError) => {
                    json!({"isError":true,"content":[{"type":"text","text":"Rate limit exceeded"}]})
                }
                "tools/call" if matches!(reply, Reply::TextSearch) => {
                    json!({"content":[{"type":"text","text":search.to_string()}]})
                }
                "tools/call" => json!({"structuredContent":search,"content":[]}),
                _ => json!({}),
            };
            let (status, response) = if matches!(reply, Reply::RateLimited) {
                ("429 Too Many Requests", "rate limited".into())
            } else if headers.starts_with("GET ") {
                ("405 Method Not Allowed", String::new())
            } else if method.starts_with("notifications/") || headers.starts_with("DELETE ") {
                ("202 Accepted", String::new())
            } else {
                (
                    "200 OK",
                    json!({"jsonrpc":"2.0","id":body["id"],"result":result}).to_string(),
                )
            };
            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{response}",
                response.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
        }
    });
    (url, requests, task)
}

#[tokio::test]
async fn config_to_handler_search_is_anonymous_and_limits_output() {
    for reply in [Reply::Search, Reply::TextSearch] {
        let (url, requests, task) = server(reply).await;
        let result = WebSearchHandler::new()
            .handle(context(), input(&url), /*progress*/ None)
            .await
            .expect("caller search");
        let ToolResultContent::Text(text) = result.content else {
            panic!("text contract")
        };
        assert_eq!(
            text,
            "Search results for: Rust language official website\n\n1. [Rust](https://www.rust-lang.org/)\nSnippet: Rust programming language.\n"
        );
        let requests = requests.lock().expect("capture lock");
        for (headers, _) in requests.iter() {
            assert!(
                headers
                    .to_lowercase()
                    .contains(&format!("user-agent: {}", USER_AGENT.to_lowercase()))
            );
            assert!(!headers.to_lowercase().contains("authorization:"));
        }
        for method in ["initialize", "tools/list", "tools/call"] {
            assert!(requests.iter().any(|(_, body)| body["method"] == method));
        }
        let call = &requests
            .iter()
            .find(|(_, body)| body["method"] == "tools/call")
            .expect("search request")
            .1;
        assert_eq!(
            // The MCP SDK also adds its own progress-token metadata.
            json!({"name": call["params"]["name"], "arguments": call["params"]["arguments"]}),
            json!({"name":"web_search", "arguments":{
                "objective":"Rust language official website", "search_queries":["Rust language official website"],
                "session_id":"1234567890abcdef1234567890abcdef"
            }})
        );
        task.abort();
    }
}

#[tokio::test]
async fn handler_preserves_configured_network_proxy() {
    let (proxy, requests, task) = server(Reply::Search).await;
    let mut ctx = context();
    ctx.network_proxy = Some(proxy);
    ctx.network_no_proxy = Some("localhost".into());
    WebSearchHandler::new()
        .handle(
            ctx,
            input("http://parallel.invalid/mcp"),
            /*progress*/ None,
        )
        .await
        .expect("proxied search");
    assert!(
        requests
            .lock()
            .expect("capture lock")
            .iter()
            .any(
                |(headers, body)| headers.starts_with("POST http://parallel.invalid/mcp ")
                    && body["method"] == "tools/call"
            )
    );
    task.abort();
}

#[tokio::test]
async fn handler_reports_mcp_and_http_rate_limit_errors() {
    for reply in [Reply::ToolError, Reply::RateLimited] {
        let (url, _, task) = server(reply).await;
        let error = WebSearchHandler::new()
            .handle(context(), input(&url), /*progress*/ None)
            .await
            .expect_err("rate limit is an error");
        let text = error.to_string();
        assert!(
            text.contains("429") || text.contains("Rate limit exceeded"),
            "{text}"
        );
        task.abort();
    }
}

#[tokio::test]
async fn handler_obeys_timeout_and_active_cancellation() {
    let (url, requests, task) = server(Reply::Slow).await;
    let mut ctx = context();
    ctx.budgets.wall_time_limit_ms = Some(50);
    let error = WebSearchHandler::new()
        .handle(ctx, input(&url), /*progress*/ None)
        .await
        .expect_err("timeout");
    assert!(error.to_string().contains("timed out") || error.to_string().contains("timeout"));
    task.abort();
    assert!(!requests.lock().expect("capture lock").is_empty());

    let (url, requests, task) = server(Reply::Slow).await;
    let ctx = context();
    let token = ctx.cancel_token.clone();
    let call = tokio::spawn(async move {
        WebSearchHandler::new()
            .handle(ctx, input(&url), /*progress*/ None)
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while requests.lock().expect("capture lock").is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("in-flight request");
    token.cancel();
    assert!(matches!(
        call.await.expect("join"),
        Err(crate::contracts::ToolCallError::Cancelled)
    ));
    task.abort();
}

#[tokio::test]
#[ignore = "uses the public anonymous Parallel endpoint"]
async fn live_config_to_handler_search_without_saved_auth() {
    let mut ctx = context();
    ctx.budgets.wall_time_limit_ms = Some(60000);
    let result = WebSearchHandler::new()
        .handle(ctx, input(super::ENDPOINT), /*progress*/ None)
        .await
        .expect("anonymous live search");
    let ToolResultContent::Text(text) = result.content else {
        panic!("text contract")
    };
    assert!(
        text.contains("https://")
            && text.contains("Snippet:")
            && text.to_lowercase().contains("rust"),
        "{text}"
    );
    println!("{text}");
}

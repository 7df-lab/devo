use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::http::header::AUTHORIZATION;
use axum::http::header::CONTENT_TYPE;
use axum::response::Response;
use axum::routing::get;
use axum::routing::post;
use devo_rmcp_client::ElicitationAction;
use devo_rmcp_client::ElicitationResponse;
use devo_rmcp_client::RmcpClient;
use futures::FutureExt as _;
use futures::StreamExt as _;
use pretty_assertions::assert_eq;
use rmcp::model::ClientCapabilities;
use rmcp::model::ElicitationCapability;
use rmcp::model::FormElicitationCapability;
use rmcp::model::Implementation;
use rmcp::model::InitializeRequestParams;
use rmcp::model::ProtocolVersion;
use serde_json::Value;
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::broadcast;

#[derive(Clone)]
struct LegacySseState {
    messages: broadcast::Sender<String>,
    saw_sse_headers: Arc<AtomicBool>,
    saw_post_headers: Arc<AtomicBool>,
}

#[tokio::test]
async fn legacy_sse_client_initializes_and_lists_tools() -> anyhow::Result<()> {
    let (server, base_url, state) = spawn_legacy_sse_server().await?;
    let client = RmcpClient::new_sse_client(
        &format!("{base_url}/sse"),
        Some("test-token".to_string()),
        Some(HashMap::from([(
            "X-Test-Header".to_string(),
            "test-header-value".to_string(),
        )])),
        /*env_http_headers*/ None,
    )
    .await?;

    client
        .initialize(
            init_params(),
            Some(Duration::from_secs(5)),
            Box::new(|_, _| {
                async {
                    Ok(ElicitationResponse {
                        action: ElicitationAction::Accept,
                        content: Some(json!({})),
                        meta: None,
                    })
                }
                .boxed()
            }),
        )
        .await?;

    let tools = client
        .list_tools(/*params*/ None, Some(Duration::from_secs(5)))
        .await?;
    assert_eq!(tools.tools, Vec::new());
    assert!(state.saw_sse_headers.load(Ordering::SeqCst));
    assert!(state.saw_post_headers.load(Ordering::SeqCst));

    client.shutdown().await;
    server.abort();
    Ok(())
}

async fn spawn_legacy_sse_server() -> anyhow::Result<(
    tokio::task::JoinHandle<std::io::Result<()>>,
    String,
    LegacySseState,
)> {
    let (messages, _) = broadcast::channel(16);
    let state = LegacySseState {
        messages,
        saw_sse_headers: Arc::new(AtomicBool::new(false)),
        saw_post_headers: Arc::new(AtomicBool::new(false)),
    };
    let router = Router::new()
        .route("/sse", get(sse_handler))
        .route("/message", post(message_handler))
        .with_state(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(async move { axum::serve(listener, router).await });

    Ok((server, format!("http://{address}"), state))
}

async fn sse_handler(State(state): State<LegacySseState>, headers: HeaderMap) -> Response {
    if has_expected_headers(&headers) {
        state.saw_sse_headers.store(true, Ordering::SeqCst);
    }

    let receiver = state.messages.subscribe();
    let endpoint = futures::stream::once(async {
        Ok::<Bytes, Infallible>(Bytes::from_static(
            b"event: endpoint\ndata: /message?sessionId=test-session\n\n",
        ))
    });
    let messages = futures::stream::unfold(receiver, |mut receiver| async move {
        match receiver.recv().await {
            Ok(message) => Some((
                Ok::<Bytes, Infallible>(Bytes::from(format!(
                    "event: message\ndata: {message}\n\n"
                ))),
                receiver,
            )),
            Err(_) => None,
        }
    });

    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/event-stream")
        .body(Body::from_stream(endpoint.chain(messages)))
        .expect("SSE response should build")
}

async fn message_handler(
    State(state): State<LegacySseState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    if has_expected_headers(&headers) {
        state.saw_post_headers.store(true, Ordering::SeqCst);
    }

    let Ok(message) = serde_json::from_slice::<Value>(&body) else {
        return StatusCode::BAD_REQUEST;
    };
    let Some(id) = message.get("id").cloned() else {
        return StatusCode::ACCEPTED;
    };
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let result = match method {
        "initialize" => json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "serverInfo": {
                "name": "legacy-sse-test",
                "version": "0.0.0-test"
            }
        }),
        "tools/list" => json!({
            "tools": []
        }),
        other => {
            let response = json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": -32601,
                    "message": format!("unknown method: {other}")
                }
            });
            let _ = state.messages.send(response.to_string());
            return StatusCode::ACCEPTED;
        }
    };
    let response = json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    });
    let _ = state.messages.send(response.to_string());
    StatusCode::ACCEPTED
}

fn has_expected_headers(headers: &HeaderMap) -> bool {
    headers
        .get(AUTHORIZATION)
        .is_some_and(|value| value.as_bytes() == b"Bearer test-token")
        && headers
            .get("x-test-header")
            .is_some_and(|value| value.as_bytes() == b"test-header-value")
}

fn init_params() -> InitializeRequestParams {
    InitializeRequestParams {
        meta: None,
        capabilities: ClientCapabilities {
            experimental: None,
            extensions: None,
            roots: None,
            sampling: None,
            elicitation: Some(ElicitationCapability {
                form: Some(FormElicitationCapability {
                    schema_validation: None,
                }),
                url: None,
            }),
            tasks: None,
        },
        client_info: Implementation {
            name: "legacy-sse-test-client".into(),
            version: "0.0.0-test".into(),
            title: Some("Legacy SSE test client".into()),
            description: None,
            icons: None,
            website_url: None,
        },
        protocol_version: ProtocolVersion::V_2025_06_18,
    }
}

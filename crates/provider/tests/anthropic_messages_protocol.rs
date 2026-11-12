use std::time::Duration;

use devo_protocol::{ModelProfileKey, ModelRequest, RequestContent, RequestMessage, ResponseContent, StreamEvent, Usage};
use devo_provider::anthropic::AnthropicProvider;
use devo_provider::error::ProviderError;
use devo_provider::ModelProviderSDK;
use futures::StreamExt;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::timeout;

fn request() -> ModelRequest {
    ModelRequest {
        model_slug: ModelProfileKey::Generic,
        model: "claude-sonnet-4-6".to_string(),
        system: None,
        messages: vec![RequestMessage { role: "user".to_string(), content: vec![RequestContent::Text { text: "Hello".to_string() }] }],
        max_tokens: 4096,
        tools: None,
        hosted_tools: Vec::new(),
        sampling: Default::default(),
        request_thinking: None,
        reasoning_effort: None,
        extra_body: None,
    }
}

async fn serve(body: String, content_type: &'static str) -> (String, tokio::task::JoinHandle<Value>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind fixture server");
    let address = listener.local_addr().expect("server address");
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept request");
        let mut request = Vec::new();
        let parsed = loop {
            let mut buffer = [0_u8; 4096];
            let read = socket.read(&mut buffer).await.expect("read request");
            assert!(read > 0, "request closed before body");
            request.extend_from_slice(&buffer[..read]);
            if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..end]);
                let length = headers.lines().find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().expect("length"))
                }).unwrap_or(0);
                if request.len() >= end + 4 + length {
                    break serde_json::from_slice(&request[end + 4..end + 4 + length]).expect("JSON body");
                }
            }
        };
        let length = body.len();
        let response = format!("HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{body}");
        socket.write_all(response.as_bytes()).await.expect("write response");
        socket.shutdown().await.expect("shutdown");
        parsed
    });
    (format!("http://{address}"), server)
}

async fn events(body: String) -> Vec<Result<StreamEvent, Value>> {
    let (url, server) = serve(body, "text/event-stream").await;
    let mut stream = AnthropicProvider::new(url).completion_stream(request()).await.expect("stream");
    let events = timeout(Duration::from_secs(5), async {
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event.map_err(|error| serde_json::to_value(error.downcast_ref::<ProviderError>().expect("typed provider error")).expect("serialize error")));
        }
        events
    }).await.expect("stream must terminate promptly");
    server.await.expect("server");
    events
}

#[tokio::test]
async fn anthropic_stream_error_is_terminal_and_retains_provider_classification() {
    for (kind, expected) in [
        ("overloaded_error", json!({"error_kind":"provider_server_error", "message":"Unavailable", "status_code":529, "provider_name":"anthropic"})),
        ("rate_limit_error", json!({"error_kind":"rate_limit_error", "message":"Unavailable", "retry_after_seconds":null, "provider_name":"anthropic"})),
        ("invalid_request_error", json!({"error_kind":"invalid_request_error", "message":"Unavailable", "details":json!({"type":"invalid_request_error", "message":"Unavailable"}).to_string()})),
    ] {
        let payload = json!({"type":"error", "error":{"type":kind,"message":"Unavailable"}});
        assert_eq!(events(format!("event: error\ndata: {payload}\n\nevent: message_stop\ndata: {{}}\n\n")).await, vec![Err(expected)]);
    }
}

#[tokio::test]
async fn anthropic_incomplete_or_non_object_tool_json_is_not_executed() {
    for partial in ["{\"path\":", "[]"] {
        let delta = json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":partial}});
        let received = events(format!(
            "event: content_block_start\ndata: {{\"index\":0,\"content_block\":{{\"type\":\"tool_use\",\"id\":\"tool_1\",\"name\":\"read\",\"input\":{{}}}}}}\n\nevent: content_block_delta\ndata: {delta}\n\nevent: content_block_stop\ndata: {{\"index\":0}}\n\nevent: message_stop\ndata: {{}}\n\n"
        )).await;
        assert!(received.last().is_some_and(Result::is_err));
        assert!(!received.iter().any(|event| matches!(event, Ok(StreamEvent::MessageDone { .. }))));
    }
}

#[tokio::test]
async fn anthropic_stream_preserves_initial_text_signed_and_redacted_thinking() {
    let blocks = [
        json!({"type":"thinking","thinking":"Summary","signature":"signature_opaque"}),
        json!({"type":"redacted_thinking","data":"encrypted_opaque"}),
        json!({"type":"text","text":"Hello"}),
    ];
    let mut body = String::from("event: message_start\ndata: {\"message\":{\"id\":\"msg_1\"}}\n\nevent: ping\ndata: {\"type\":\"ping\"}\n\nevent: future_event\ndata: {}\n\n");
    for (index, block) in blocks.iter().enumerate() {
        let start = json!({"index":index,"content_block":block});
        body.push_str(&format!("event: content_block_start\ndata: {start}\n\nevent: content_block_stop\ndata: {{\"index\":{index}}}\n\n"));
    }
    body.push_str("event: message_stop\ndata: {}\n\n");
    let received = events(body).await;
    let Some(Ok(StreamEvent::MessageDone { response })) = received.last() else { panic!("expected final response: {received:?}") };
    assert_eq!(response.content, vec![
        ResponseContent::ProviderReasoning { provider: "anthropic".to_string(), payload: blocks[0].clone() },
        ResponseContent::ProviderReasoning { provider: "anthropic".to_string(), payload: blocks[1].clone() },
        ResponseContent::Text("Hello".to_string()),
    ]);
    assert!(received.contains(&Ok(StreamEvent::TextDelta { index: 2, text: "Hello".to_string() })));
    assert!(received.contains(&Ok(StreamEvent::ReasoningDelta { index: 0, text: "Summary".to_string() })));
}

#[tokio::test]
async fn anthropic_redacted_thinking_survives_completion_and_request_replay() {
    let blocks = json!([
        {"type":"thinking","thinking":"Summary","signature":"signed_opaque"},
        {"type":"redacted_thinking","data":"encrypted_opaque"},
        {"type":"tool_use","id":"tool_1","name":"read","input":{"path":"README.md"}}
    ]);
    let response_body = json!({"id":"msg_1","content":blocks,"stop_reason":"tool_use"}).to_string();
    let (url, first_server) = serve(response_body.clone(), "application/json").await;
    let response = AnthropicProvider::new(url).completion(request()).await.expect("completion");
    first_server.await.expect("first server");
    assert_eq!(response.content[1], ResponseContent::ProviderReasoning { provider: "anthropic".to_string(), payload: blocks[1].clone() });
    let mut replay = request();
    replay.messages.push(RequestMessage { role: "assistant".to_string(), content: response.content.into_iter().map(|block| match block {
        ResponseContent::ProviderReasoning { provider, payload } => RequestContent::ProviderReasoning { provider, payload },
        ResponseContent::ToolUse { id, name, input } => RequestContent::ToolUse { id, name, input },
        other => panic!("unexpected block: {other:?}"),
    }).collect() });
    replay.messages.push(RequestMessage { role: "user".to_string(), content: vec![RequestContent::ToolResult { tool_use_id: "tool_1".to_string(), content: "Readme".to_string(), is_error: None }] });
    let (url, server) = serve(response_body, "application/json").await;
    AnthropicProvider::new(url).completion(replay).await.expect("replay completion");
    assert_eq!(server.await.expect("server")["messages"][1]["content"], blocks);
}

#[tokio::test]
async fn anthropic_usage_preserves_cache_totals_and_reasoning_decomposition() {
    let usage = json!({"input_tokens":10,"cache_creation_input_tokens":20,"cache_read_input_tokens":30,"output_tokens":40,"output_tokens_details":{"thinking_tokens":25}});
    let expected = Usage {
        reported: true, input_tokens: 60, output_tokens: 40,
        cache_creation_input_tokens: Some(20), cache_read_input_tokens: Some(30),
        reasoning_output_tokens: Some(25), ..Usage::default()
    }.with_field_coverage(/*input_tokens*/ Some(10), /*output_tokens*/ Some(40));
    let (url, server) = serve(json!({"id":"msg_1","content":[],"usage":usage}).to_string(), "application/json").await;
    assert_eq!(AnthropicProvider::new(url).completion(request()).await.expect("completion").usage, expected);
    server.await.expect("server");
    let body = format!("event: message_start\ndata: {{\"message\":{{\"id\":\"msg_1\",\"usage\":{{\"input_tokens\":10,\"cache_creation_input_tokens\":20,\"cache_read_input_tokens\":30,\"output_tokens\":1}}}}}}\n\nevent: message_delta\ndata: {{\"usage\":{usage}}}\n\nevent: message_stop\ndata: {{}}\n\n");
    let received = events(body).await;
    let Some(Ok(StreamEvent::MessageDone { response })) = received.last() else { panic!("missing message: {received:?}") };
    assert_eq!(response.usage, expected);
}

#[tokio::test]
async fn anthropic_truncated_stream_never_reports_message_done() {
    let received = events("event: message_start\ndata: {\"message\":{\"id\":\"msg_1\"}}\n\n".to_string()).await;
    assert!(received.last().is_some_and(Result::is_err));
    assert!(!received.iter().any(|event| matches!(event, Ok(StreamEvent::MessageDone { .. }))));
}

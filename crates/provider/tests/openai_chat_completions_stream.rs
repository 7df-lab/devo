use std::io;

use devo_protocol::{ModelRequest, RequestContent, RequestMessage, ResponseContent, StreamEvent};
use devo_provider::ModelProviderSDK;
use devo_provider::openai::OpenAIProvider;
use futures::StreamExt;
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[tokio::test]
async fn chat_stream_buffers_tool_arguments_until_identity_arrives() {
    let chunks = vec![
        sse_data(json!({
            "id": "chatcmpl-tool",
            "object": "chat.completion.chunk",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": {
                            "arguments": "{\"path\""
                        }
                    }]
                },
                "finish_reason": null
            }]
        })),
        sse_data(json!({
            "id": "chatcmpl-tool",
            "object": "chat.completion.chunk",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_read",
                        "type": "function",
                        "function": {
                            "name": "read"
                        }
                    }]
                },
                "finish_reason": null
            }]
        })),
        sse_data(json!({
            "id": "chatcmpl-tool",
            "object": "chat.completion.chunk",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": {
                            "arguments": ":\"README.md\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        })),
        "data: [DONE]\n\n".to_string(),
    ];
    let base_url = spawn_sse_server(chunks).await;
    let provider = OpenAIProvider::new(base_url);

    let mut stream = provider
        .completion_stream(minimal_request())
        .await
        .expect("openai chat stream");
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event.expect("stream event"));
    }

    let starts = events
        .iter()
        .filter_map(|event| match event {
            StreamEvent::ToolCallStart {
                index,
                id,
                name,
                input,
            } => Some((*index, id.clone(), name.clone(), input.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        starts,
        vec![(1, "call_read".to_string(), "read".to_string(), json!({}))]
    );

    let streamed_arguments = events
        .iter()
        .filter_map(|event| match event {
            StreamEvent::ToolCallInputDelta { partial_json, .. } => Some(partial_json.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert_eq!(streamed_arguments, r#"{"path":"README.md"}"#);

    let response = events.iter().find_map(|event| match event {
        StreamEvent::MessageDone { response } => Some(response),
        _ => None,
    });
    let Some(response) = response else {
        panic!("expected MessageDone event");
    };
    assert_eq!(
        response.content,
        vec![ResponseContent::ToolUse {
            id: "call_read".to_string(),
            name: "read".to_string(),
            input: json!({ "path": "README.md" }),
        }]
    );
}

#[tokio::test]
async fn chat_stream_uses_array_position_when_tool_call_index_is_missing() {
    let events = collect_chat_stream_events(vec![
        sse_data(json!({
            "id": "chatcmpl-tool",
            "object": "chat.completion.chunk",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [
                        {
                            "id": "call_city",
                            "type": "function",
                            "function": {
                                "name": "get_city",
                                "arguments": "{\"city\":\"Boston\"}"
                            }
                        },
                        {
                            "id": "call_time",
                            "type": "function",
                            "function": {
                                "name": "get_time",
                                "arguments": "{\"zone\":\"UTC\"}"
                            }
                        }
                    ]
                },
                "finish_reason": "tool_calls"
            }]
        })),
        "data: [DONE]\n\n".to_string(),
    ])
    .await;

    let starts = events
        .iter()
        .filter_map(|event| match event {
            StreamEvent::ToolCallStart {
                index, id, name, ..
            } => Some((*index, id.clone(), name.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        starts,
        vec![
            (1, "call_city".to_string(), "get_city".to_string()),
            (2, "call_time".to_string(), "get_time".to_string()),
        ]
    );

    let response = message_done_response(&events);
    assert_eq!(
        response.content,
        vec![
            ResponseContent::ToolUse {
                id: "call_city".to_string(),
                name: "get_city".to_string(),
                input: json!({ "city": "Boston" }),
            },
            ResponseContent::ToolUse {
                id: "call_time".to_string(),
                name: "get_time".to_string(),
                input: json!({ "zone": "UTC" }),
            },
        ]
    );
}

#[tokio::test]
async fn chat_stream_handles_max_tool_call_index_without_panicking() {
    let events = collect_chat_stream_events(vec![
        sse_data(json!({
            "id": "chatcmpl-tool",
            "object": "chat.completion.chunk",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": u32::MAX,
                        "id": "call_large",
                        "type": "function",
                        "function": {
                            "name": "read",
                            "arguments": "{\"path\":\"README.md\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        })),
        "data: [DONE]\n\n".to_string(),
    ])
    .await;

    let expected_index =
        usize::try_from(u32::MAX).map_or(usize::MAX, |index| index.saturating_add(1));
    let starts = events
        .iter()
        .filter_map(|event| match event {
            StreamEvent::ToolCallStart {
                index, id, name, ..
            } => Some((*index, id.clone(), name.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        starts,
        vec![(expected_index, "call_large".to_string(), "read".to_string())]
    );

    let response = message_done_response(&events);
    assert_eq!(
        response.content,
        vec![ResponseContent::ToolUse {
            id: "call_large".to_string(),
            name: "read".to_string(),
            input: json!({ "path": "README.md" }),
        }]
    );
}

async fn collect_chat_stream_events(chunks: Vec<String>) -> Vec<StreamEvent> {
    let base_url = spawn_sse_server(chunks).await;
    let provider = OpenAIProvider::new(base_url);

    let mut stream = provider
        .completion_stream(minimal_request())
        .await
        .expect("openai chat stream");
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event.expect("stream event"));
    }
    events
}

fn message_done_response(events: &[StreamEvent]) -> &devo_protocol::ModelResponse {
    let Some(response) = events.iter().find_map(|event| match event {
        StreamEvent::MessageDone { response } => Some(response),
        _ => None,
    }) else {
        panic!("expected MessageDone event");
    };
    response
}

async fn spawn_sse_server(chunks: Vec<String>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test server");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept request");
        read_http_request(&mut socket).await.expect("read request");
        let body = chunks.concat();
        let response = format!(
            "HTTP/1.1 200 OK\r\n\
             content-type: text/event-stream\r\n\
             cache-control: no-cache\r\n\
             content-length: {}\r\n\
             connection: close\r\n\
             \r\n{}",
            body.len(),
            body
        );
        socket
            .write_all(response.as_bytes())
            .await
            .expect("write response");
    });
    format!("http://{addr}")
}

async fn read_http_request(socket: &mut tokio::net::TcpStream) -> io::Result<String> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 1024];
    loop {
        let count = socket.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

fn sse_data(value: serde_json::Value) -> String {
    format!("data: {value}\n\n")
}

fn minimal_request() -> ModelRequest {
    ModelRequest {
        model: "test-model".to_string(),
        system: None,
        messages: vec![RequestMessage {
            role: "user".to_string(),
            content: vec![RequestContent::Text {
                text: "Read README.md.".to_string(),
            }],
        }],
        max_tokens: 16,
        tools: None,
        hosted_tools: Vec::new(),
        sampling: Default::default(),
        request_thinking: None,
        reasoning_effort: None,
        extra_body: None,
    }
}

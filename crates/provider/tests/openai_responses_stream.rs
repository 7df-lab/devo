use std::io;

use devo_protocol::{
    ModelRequest, ModelResponse, RequestContent, RequestMessage, ResponseContent, ResponseExtra,
    ResponseMetadata, StopReason, StreamEvent, Usage,
};
use devo_provider::ModelProviderSDK;
use devo_provider::openai::OpenAIResponsesProvider;
use futures::StreamExt;
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[tokio::test]
async fn responses_stream_completion_closes_tagged_reasoning_before_message_done() {
    let (base_url, _capture) = spawn_sse_server(OPENAI_RESPONSES_REASONING_COMPLETED_SSE).await;
    let provider = OpenAIResponsesProvider::new(base_url);

    let mut stream = provider
        .completion_stream(minimal_request())
        .await
        .expect("responses stream");
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event.expect("stream event"));
    }

    assert_eq!(
        events,
        vec![
            StreamEvent::ReasoningStart { index: 1 },
            StreamEvent::ReasoningDelta {
                index: 1,
                text: "plan".to_string()
            },
            StreamEvent::ReasoningDone { index: 1 },
            StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "resp_reasoning".to_string(),
                    content: vec![ResponseContent::Text("final".to_string())],
                    stop_reason: Some(StopReason::EndTurn),
                    usage: Usage {
                        input_tokens: 1,
                        output_tokens: 1,
                        cache_creation_input_tokens: None,
                        cache_read_input_tokens: None,
                    },
                    metadata: ResponseMetadata {
                        extras: vec![ResponseExtra::ReasoningText {
                            text: "plan".to_string()
                        }],
                    },
                },
            },
        ]
    );
}

#[tokio::test]
async fn responses_stream_done_flushes_buffered_partial_tag_as_text_delta() {
    let (base_url, _capture) = spawn_sse_server(OPENAI_RESPONSES_PARTIAL_TAG_DONE_SSE).await;
    let provider = OpenAIResponsesProvider::new(base_url);

    let mut stream = provider
        .completion_stream(minimal_request())
        .await
        .expect("responses stream");
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event.expect("stream event"));
    }

    assert_eq!(
        events,
        vec![
            StreamEvent::TextStart { index: 0 },
            StreamEvent::TextDelta {
                index: 0,
                text: "<thi".to_string()
            },
            StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "resp_done".to_string(),
                    content: vec![ResponseContent::Text("<thi".to_string())],
                    stop_reason: Some(StopReason::EndTurn),
                    usage: Usage::default(),
                    metadata: ResponseMetadata::default(),
                },
            },
        ]
    );
}

#[tokio::test]
async fn responses_stream_routes_function_arguments_by_output_item_id() {
    let (base_url, _capture) = spawn_sse_server(OPENAI_RESPONSES_FUNCTION_ARGUMENTS_SSE).await;
    let provider = OpenAIResponsesProvider::new(base_url);

    let mut stream = provider
        .completion_stream(minimal_request())
        .await
        .expect("responses stream");
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event.expect("stream event"));
    }

    assert_eq!(
        events,
        vec![
            StreamEvent::ToolCallStart {
                index: 1,
                id: "call_first".to_string(),
                name: "read_file".to_string(),
                input: json!({}),
            },
            StreamEvent::ToolCallStart {
                index: 2,
                id: "call_second".to_string(),
                name: "write_file".to_string(),
                input: json!({}),
            },
            StreamEvent::ToolCallInputDelta {
                index: 2,
                partial_json: "{\"path\":\"b.md\"}".to_string(),
            },
            StreamEvent::ToolCallInputDelta {
                index: 1,
                partial_json: "{\"path\":\"a.md\"}".to_string(),
            },
            StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "resp_tools".to_string(),
                    content: vec![
                        ResponseContent::ToolUse {
                            id: "call_first".to_string(),
                            name: "read_file".to_string(),
                            input: json!({"path": "a.md"}),
                        },
                        ResponseContent::ToolUse {
                            id: "call_second".to_string(),
                            name: "write_file".to_string(),
                            input: json!({"path": "b.md"}),
                        },
                    ],
                    stop_reason: Some(StopReason::EndTurn),
                    usage: Usage::default(),
                    metadata: ResponseMetadata::default(),
                },
            },
        ]
    );
}

#[tokio::test]
async fn responses_stream_fallback_preserves_mixed_tool_order_and_hosted_completion() {
    let (base_url, _capture) = spawn_sse_server(OPENAI_RESPONSES_MIXED_TOOLS_DONE_SSE).await;
    let provider = OpenAIResponsesProvider::new(base_url);

    let mut stream = provider
        .completion_stream(minimal_request())
        .await
        .expect("responses stream");
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event.expect("stream event"));
    }

    assert_eq!(
        events,
        vec![
            StreamEvent::HostedToolCallStart {
                index: 1,
                id: "ws_first".to_string(),
                name: "web_search".to_string(),
                input: json!({"query": "Rust async docs"}),
            },
            StreamEvent::ToolCallStart {
                index: 2,
                id: "call_second".to_string(),
                name: "read_file".to_string(),
                input: json!({}),
            },
            StreamEvent::ToolCallInputDelta {
                index: 2,
                partial_json: "{\"path\":\"README.md\"}".to_string(),
            },
            StreamEvent::HostedToolCallDone {
                index: 1,
                id: "ws_first".to_string(),
                name: "web_search".to_string(),
                input: json!({"query": "Rust async docs"}),
                output: Some(json!([
                    {
                        "title": "Async Rust",
                        "url": "https://example.test/rust"
                    }
                ])),
                status: Some("completed".to_string()),
            },
            StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "resp_mixed".to_string(),
                    content: vec![
                        ResponseContent::HostedToolUse {
                            id: "ws_first".to_string(),
                            name: "web_search".to_string(),
                            input: json!({"query": "Rust async docs"}),
                            output: Some(json!([
                                {
                                    "title": "Async Rust",
                                    "url": "https://example.test/rust"
                                }
                            ])),
                            status: Some("completed".to_string()),
                        },
                        ResponseContent::ToolUse {
                            id: "call_second".to_string(),
                            name: "read_file".to_string(),
                            input: json!({"path": "README.md"}),
                        },
                    ],
                    stop_reason: Some(StopReason::EndTurn),
                    usage: Usage::default(),
                    metadata: ResponseMetadata::default(),
                },
            },
        ]
    );
}

async fn spawn_sse_server(response: &'static str) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test server");
    let addr = listener.local_addr().expect("local addr");
    let handle = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept request");
        let request = read_http_request(&mut socket).await.expect("read request");
        socket
            .write_all(response.as_bytes())
            .await
            .expect("write response");
        request
    });

    (format!("http://{addr}"), handle)
}

async fn read_http_request(socket: &mut TcpStream) -> io::Result<String> {
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

fn minimal_request() -> ModelRequest {
    ModelRequest {
        model: "test-model".to_string(),
        system: None,
        messages: vec![RequestMessage {
            role: "user".to_string(),
            content: vec![RequestContent::Text {
                text: "Reply with OK only.".to_string(),
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

const OPENAI_RESPONSES_REASONING_COMPLETED_SSE: &str = concat!(
    "HTTP/1.1 200 OK\r\n",
    "content-type: text/event-stream\r\n",
    "cache-control: no-cache\r\n",
    "connection: close\r\n",
    "\r\n",
    "event: response.output_text.delta\n",
    "data: {\"id\":\"resp_reasoning\",\"delta\":\"<think>plan\"}\n\n",
    "event: response.completed\n",
    "data: {\"id\":\"resp_reasoning\",\"response\":{\"id\":\"resp_reasoning\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"final\"}]}],\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\n",
);

const OPENAI_RESPONSES_PARTIAL_TAG_DONE_SSE: &str = concat!(
    "HTTP/1.1 200 OK\r\n",
    "content-type: text/event-stream\r\n",
    "cache-control: no-cache\r\n",
    "connection: close\r\n",
    "\r\n",
    "event: response.output_text.delta\n",
    "data: {\"id\":\"resp_done\",\"delta\":\"<thi\"}\n\n",
    "data: [DONE]\n\n",
);

const OPENAI_RESPONSES_FUNCTION_ARGUMENTS_SSE: &str = concat!(
    "HTTP/1.1 200 OK\r\n",
    "content-type: text/event-stream\r\n",
    "cache-control: no-cache\r\n",
    "connection: close\r\n",
    "\r\n",
    "event: response.output_item.added\n",
    "data: {\"id\":\"resp_tools\",\"item\":{\"id\":\"fc_first\",\"type\":\"function_call\",\"call_id\":\"call_first\",\"name\":\"read_file\",\"arguments\":\"\"}}\n\n",
    "event: response.output_item.added\n",
    "data: {\"id\":\"resp_tools\",\"item\":{\"id\":\"fc_second\",\"type\":\"function_call\",\"call_id\":\"call_second\",\"name\":\"write_file\",\"arguments\":\"\"}}\n\n",
    "event: response.function_call_arguments.delta\n",
    "data: {\"id\":\"resp_tools\",\"item_id\":\"fc_second\",\"delta\":\"{\\\"path\\\":\\\"b.md\\\"}\"}\n\n",
    "event: response.function_call_arguments.delta\n",
    "data: {\"id\":\"resp_tools\",\"item_id\":\"fc_first\",\"delta\":\"{\\\"path\\\":\\\"a.md\\\"}\"}\n\n",
    "event: response.completed\n",
    "data: {\"id\":\"resp_tools\"}\n\n",
);

const OPENAI_RESPONSES_MIXED_TOOLS_DONE_SSE: &str = concat!(
    "HTTP/1.1 200 OK\r\n",
    "content-type: text/event-stream\r\n",
    "cache-control: no-cache\r\n",
    "connection: close\r\n",
    "\r\n",
    "event: response.output_item.added\n",
    "data: {\"id\":\"resp_mixed\",\"item\":{\"id\":\"ws_first\",\"type\":\"web_search_call\",\"status\":\"in_progress\",\"action\":{\"type\":\"search\",\"query\":\"Rust async docs\"}}}\n\n",
    "event: response.output_item.added\n",
    "data: {\"id\":\"resp_mixed\",\"item\":{\"id\":\"fc_second\",\"type\":\"function_call\",\"call_id\":\"call_second\",\"name\":\"read_file\",\"arguments\":\"\"}}\n\n",
    "event: response.function_call_arguments.delta\n",
    "data: {\"id\":\"resp_mixed\",\"item_id\":\"fc_second\",\"delta\":\"{\\\"path\\\":\\\"README.md\\\"}\"}\n\n",
    "event: response.output_item.done\n",
    "data: {\"id\":\"resp_mixed\",\"item\":{\"id\":\"ws_first\",\"type\":\"web_search_call\",\"status\":\"completed\",\"action\":{\"type\":\"search\",\"query\":\"Rust async docs\"},\"results\":[{\"title\":\"Async Rust\",\"url\":\"https://example.test/rust\"}]}}\n\n",
    "event: response.completed\n",
    "data: {\"id\":\"resp_mixed\"}\n\n",
);

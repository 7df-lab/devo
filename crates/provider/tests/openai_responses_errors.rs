use std::time::Duration;

use devo_protocol::{
    ModelProfileKey, ModelRequest, ModelResponse, RequestContent, RequestMessage, ResponseContent,
    ResponseMetadata, StopReason, StreamEvent, Usage,
};
use devo_provider::error::ProviderError;
use devo_provider::openai::OpenAIResponsesProvider;
use devo_provider::{ModelProviderSDK, ProviderHttpOptions};
use futures::StreamExt;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio::time::timeout;

#[tokio::test]
async fn nested_quota_error_then_failed_emits_only_the_first_error() {
    let created = json!({
        "type": "response.created",
        "response": {"id": "resp_no_credits", "status": "in_progress"}
    });
    let in_progress = json!({
        "type": "response.in_progress",
        "response": {"id": "resp_no_credits", "status": "in_progress"}
    });
    let first = json!({
        "type": "error",
        "error": {
            "type": "insufficient_quota",
            "code": "credit_balance_exhausted",
            "message": "You have no credits remaining. Please add credits to continue."
        }
    });
    let failed = json!({
        "type": "response.failed",
        "response": {"id": "resp_no_credits", "status": "failed", "error": {
            "type": "insufficient_quota",
            "code": "credit_balance_exhausted",
            "message": "Repeated terminal error"
        }}
    });
    let events = collect_events(format!(
        "event: response.created\ndata: {created}\n\n\
         event: response.in_progress\ndata: {in_progress}\n\n\
         event: error\ndata: {first}\n\n\
         event: response.failed\ndata: {failed}\n\n"
    ))
    .await;

    assert_eq!(
        events,
        vec![Err(json!({
            "error_kind": "quota_exceeded_error",
            "message": "You have no credits remaining. Please add credits to continue.",
            "provider_name": "openai"
        }))]
    );
}

#[tokio::test]
async fn failed_without_preceding_error_emits_the_response_error() {
    for code in ["insufficient_quota", "credit_balance_exhausted"] {
        let payload = json!({
            "type": "response.failed",
            "response": {"id": "resp_failed", "status": "failed", "error": {
                "code": code, "message": "Account unavailable"
            }}
        });
        assert_eq!(
            collect_events(format!("event: response.failed\ndata: {payload}\n\n")).await,
            vec![Err(json!({
                "error_kind": "quota_exceeded_error",
                "message": "Account unavailable",
                "provider_name": "openai"
            }))],
            "standalone response.failed: {code}"
        );
    }
}

#[tokio::test]
async fn error_accepts_top_level_code_and_message() {
    for code in ["insufficient_quota", "credit_balance_exhausted"] {
        let payload = json!({"type": "error", "code": code, "message": "Account unavailable"});
        assert_eq!(
            collect_events(format!("event: error\ndata: {payload}\n\n")).await,
            vec![Err(json!({
                "error_kind": "quota_exceeded_error",
                "message": "Account unavailable",
                "provider_name": "openai"
            }))],
            "top-level error: {code}"
        );
    }
}

#[tokio::test]
async fn absent_message_and_empty_event_fields_fall_back_to_json_type() {
    for event_field in ["", "event: message\n", "event:\n"] {
        for payload in [
            json!({"type": "error", "code": "insufficient_quota", "message": "Account unavailable"}),
            json!({"type": "response.failed", "response": {"error": {
                "code": "credit_balance_exhausted", "message": "Account unavailable"
            }}}),
        ] {
            assert_eq!(
                collect_events(format!("{event_field}data: {payload}\n\n")).await,
                vec![Err(json!({
                    "error_kind": "quota_exceeded_error",
                    "message": "Account unavailable",
                    "provider_name": "openai"
                }))],
                "event field {event_field:?}, JSON payload {payload}"
            );
        }
    }
}

#[tokio::test]
async fn explicit_event_takes_precedence_over_json_type() {
    let payload = json!({
        "type": "error", "id": "resp_precedence", "delta": "Hello",
        "error": {"code": "insufficient_quota", "message": "Must not be treated as an error"}
    });
    assert_eq!(
        collect_events(format!(
            "event: response.output_text.delta\ndata: {payload}\n\n"
        ))
        .await,
        vec![
            Ok(StreamEvent::TextStart { index: 0 }),
            Ok(StreamEvent::TextDelta {
                index: 0,
                text: "Hello".to_string()
            }),
            Ok(StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "resp_precedence".to_string(),
                    content: vec![ResponseContent::Text("Hello".to_string())],
                    stop_reason: Some(StopReason::EndTurn),
                    usage: Usage::default(),
                    metadata: ResponseMetadata::default(),
                }
            }),
        ]
    );

    let payload = json!({
        "type": "response.completed", "response": {"error": {
            "code": "credit_balance_exhausted", "message": "Explicit failure"
        }}
    });
    assert_eq!(
        collect_events(format!("event: response.failed\ndata: {payload}\n\n")).await,
        vec![Err(json!({
            "error_kind": "quota_exceeded_error",
            "message": "Explicit failure",
            "provider_name": "openai"
        }))]
    );
}

#[tokio::test]
async fn failed_without_error_details_uses_failure_fallback() {
    for payload in [
        json!({"type": "response.failed"}),
        json!({"type": "response.failed", "response": {"status": "failed"}}),
        json!({"type": "response.failed", "response": {"error": null}}),
        json!({"type": "response.failed", "response": {"error": {}}}),
        json!({"type": "response.failed", "response": {"error": {"message": "  "}}}),
    ] {
        assert_eq!(
            collect_events(format!("event: response.failed\ndata: {payload}\n\n")).await,
            vec![Err(json!({
                "error_kind": "unknown_error",
                "message": "OpenAI Responses request failed without error details",
                "status_code": null
            }))],
            "missing failure details: {payload}"
        );
    }
}

#[tokio::test]
async fn error_without_message_uses_stream_error_fallback() {
    for payload in [
        json!({"type": "error"}),
        json!({"type": "error", "error": {}}),
        json!({"type": "error", "message": "  "}),
    ] {
        assert_eq!(
            collect_events(format!("event: error\ndata: {payload}\n\n")).await,
            vec![Err(json!({
                "error_kind": "unknown_error",
                "message": "OpenAI Responses stream returned an error without details",
                "status_code": null
            }))],
            "missing error message: {payload}"
        );
    }
}

#[tokio::test]
async fn null_and_malformed_terminal_payloads_never_become_success() {
    for (event, fallback) in [
        (
            "error",
            "OpenAI Responses stream returned an error without details",
        ),
        (
            "response.failed",
            "OpenAI Responses request failed without error details",
        ),
    ] {
        for malformed in [
            json!(null),
            json!(false),
            json!(17),
            json!("broken"),
            json!([]),
            json!([{}]),
        ] {
            for payload in [
                malformed.clone(),
                json!({"error": malformed, "response": {"error": malformed}}),
                json!({"error": malformed, "response": malformed}),
            ] {
                assert_eq!(
                    collect_events(format!(
                        "event: {event}\ndata: {payload}\n\n\
                         event: response.completed\ndata: {{\"id\":\"must_not_succeed\"}}\n\n"
                    ))
                    .await,
                    vec![Err(json!({
                        "error_kind": "unknown_error",
                        "message": fallback,
                        "status_code": null
                    }))],
                    "terminal event {event} with malformed payload {payload}"
                );
            }
        }
    }
}

#[tokio::test]
async fn malformed_error_fields_preserve_failure_classification_and_valid_message() {
    let original_message = "  Original malformed-field error: δ\n";
    for (event, fallback) in [
        (
            "error",
            "OpenAI Responses stream returned an error without details",
        ),
        (
            "response.failed",
            "OpenAI Responses request failed without error details",
        ),
    ] {
        let cases = [
            (
                json!({"message": null, "type": null, "code": null}),
                json!({
                    "error_kind": "unknown_error", "message": fallback, "status_code": null
                }),
            ),
            (
                json!({"message": 17, "type": [], "code": {}}),
                json!({
                    "error_kind": "unknown_error", "message": fallback, "status_code": null
                }),
            ),
            (
                json!({"message": [], "type": false, "code": []}),
                json!({
                    "error_kind": "unknown_error", "message": fallback, "status_code": null
                }),
            ),
            (
                json!({"message": {}, "type": 17, "code": false}),
                json!({
                    "error_kind": "unknown_error", "message": fallback, "status_code": null
                }),
            ),
            (
                json!({"message": original_message, "type": {}, "code": []}),
                json!({
                    "error_kind": "unknown_error", "message": original_message, "status_code": null
                }),
            ),
            (
                json!({"message": null, "type": [], "code": "insufficient_quota"}),
                json!({
                    "error_kind": "quota_exceeded_error", "message": fallback, "provider_name": "openai"
                }),
            ),
            (
                json!({"message": false, "type": {}, "code": "credit_balance_exhausted"}),
                json!({
                    "error_kind": "quota_exceeded_error", "message": fallback, "provider_name": "openai"
                }),
            ),
            (
                json!({"message": original_message, "type": [], "code": 429}),
                json!({
                    "error_kind": "rate_limit_error", "message": original_message,
                    "retry_after_seconds": null, "provider_name": "openai"
                }),
            ),
            (
                json!({"message": original_message, "type": "server_error", "code": {}}),
                json!({
                    "error_kind": "provider_server_error", "message": original_message,
                    "status_code": null, "provider_name": "openai"
                }),
            ),
        ];
        for (error, expected) in cases {
            let payloads = if event == "error" {
                vec![error.clone(), json!({"error": error})]
            } else {
                vec![json!({"response": {"error": error}})]
            };
            for payload in payloads {
                assert_eq!(
                    collect_events(format!("event: {event}\ndata: {payload}\n\n")).await,
                    vec![Err(expected.clone())],
                    "terminal event {event} with malformed error fields {payload}"
                );
            }
        }
    }
}

#[tokio::test]
async fn failure_after_text_does_not_emit_a_successful_message_done() {
    let failed = json!({
        "type": "response.failed", "response": {"error": {
            "code": "insufficient_quota", "message": "Account unavailable"
        }}
    });
    assert_eq!(
        collect_events(format!(
            "event: response.output_text.delta\n\
             data: {{\"id\":\"resp_partial\",\"delta\":\"Partial answer\"}}\n\n\
             event: response.failed\ndata: {failed}\n\n"
        ))
        .await,
        vec![
            Ok(StreamEvent::TextStart { index: 0 }),
            Ok(StreamEvent::TextDelta {
                index: 0,
                text: "Partial answer".to_string()
            }),
            Err(json!({
                "error_kind": "quota_exceeded_error",
                "message": "Account unavailable",
                "provider_name": "openai"
            })),
        ]
    );
}

#[tokio::test]
async fn non_quota_errors_keep_existing_classifications_and_unknown_message() {
    let message = "  Original upstream failure: δ / opaque-detail\n";
    let cases = [
        (
            json!({"code": 401}),
            json!({
                "error_kind": "authentication_error", "message": message,
                "provider_name": "openai", "status_code": 401
            }),
        ),
        (
            json!({"code": 404}),
            json!({
                "error_kind": "model_not_found_error", "message": message,
                "model_name": "test-model"
            }),
        ),
        (
            json!({"code": 408}),
            json!({
                "error_kind": "provider_timeout_error", "message": message,
                "provider_name": "openai"
            }),
        ),
        (
            json!({"code": 429}),
            json!({
                "error_kind": "rate_limit_error", "message": message,
                "provider_name": "openai", "retry_after_seconds": null
            }),
        ),
        (
            json!({"code": "503"}),
            json!({
                "error_kind": "provider_server_error", "message": message,
                "provider_name": "openai", "status_code": 503
            }),
        ),
        (
            json!({"type": "rate_limit_error"}),
            json!({
                "error_kind": "rate_limit_error", "message": message,
                "provider_name": "openai", "retry_after_seconds": null
            }),
        ),
        (
            json!({"type": "server_error"}),
            json!({
                "error_kind": "provider_server_error", "message": message,
                "provider_name": "openai", "status_code": null
            }),
        ),
        (
            json!({"code": "context_length_exceeded"}),
            json!({
                "error_kind": "context_limit_error", "message": message,
                "current_tokens": null, "limit": null
            }),
        ),
        (
            json!({"type": "opaque_failure", "code": "unknown_vendor_code"}),
            json!({
                "error_kind": "unknown_error", "message": message, "status_code": null
            }),
        ),
    ];
    for (mut error, expected) in cases {
        error["message"] = json!(message);
        for (event_field, payload) in [
            ("event: error\n", json!({"type": "error", "error": error})),
            (
                "event: response.failed\n",
                json!({"type": "response.failed", "response": {"error": error}}),
            ),
        ] {
            assert_eq!(
                collect_events(format!("{event_field}data: {payload}\n\n")).await,
                vec![Err(expected.clone())],
                "existing classification for {payload}"
            );
        }
    }
}

#[tokio::test]
async fn completion_fold_propagates_failure_instead_of_returning_partial_text() {
    for event_field in ["event: error\n", "event: response.failed\n"] {
        let error = json!({"code": "insufficient_quota", "message": "Account unavailable"});
        let payload = json!({"error": error, "response": {"error": error}});
        let (provider, server) = spawn_provider(format!(
            "event: response.output_text.delta\n\
             data: {{\"id\":\"resp_partial\",\"delta\":\"Partial answer\"}}\n\n\
             {event_field}data: {payload}\n\n"
        ))
        .await;
        let response = timeout(
            Duration::from_secs(10),
            provider.completion(minimal_request()),
        )
        .await
        .expect("completion must terminate")
        .map_err(|error| {
            serde_json::to_value(
                error
                    .downcast_ref::<ProviderError>()
                    .expect("typed provider error"),
            )
            .expect("serialize provider error")
        });
        server.await.expect("local SSE server completed");
        assert_eq!(
            response,
            Err(json!({
                "error_kind": "quota_exceeded_error",
                "message": "Account unavailable",
                "provider_name": "openai"
            }))
        );
    }
}

#[tokio::test]
async fn ordinary_eof_does_not_complete_streamed_text() {
    let events = collect_events(
        concat!(
            "event: response.output_text.delta\n",
            "data: {\"id\":\"resp_eof\",\"delta\":\"Hello\"}\n\n",
        )
        .to_string(),
    )
    .await;
    assert_eq!(
        events,
        vec![
            Ok(StreamEvent::TextStart { index: 0 }),
            Ok(StreamEvent::TextDelta { index: 0, text: "Hello".to_string() }),
            Err(json!({
                "error_kind": "stream_error",
                "message": "openai responses stream ended without a terminal event",
                "bytes_received": null
            })),
        ]
    );
}

#[tokio::test]
async fn incomplete_preserves_reason_usage_and_partial_content() {
    for (reason, stop_reason) in [("max_output_tokens", StopReason::MaxTokens), ("content_filter", StopReason::StopSequence)] {
        let events = collect_events(format!("data: {}\n\n", json!({
            "type": "response.incomplete",
            "response": {
                "id": "resp_incomplete", "status": "incomplete",
                "incomplete_details": {"reason": reason},
                "output": [{"type":"message","content":[{"type":"output_text","text":"partial"}]}],
                "usage": {"input_tokens": 4, "output_tokens": 2}
            }
        }))).await;
        assert_eq!(events, vec![Ok(StreamEvent::MessageDone { response: ModelResponse {
            id: "resp_incomplete".to_string(),
            content: vec![ResponseContent::Text("partial".to_string())],
            stop_reason: Some(stop_reason),
            usage: Usage { reported: true, input_tokens: 4, output_tokens: 2, ..Usage::default() }
                .with_field_coverage(Some(4), Some(2)),
            metadata: ResponseMetadata::default(),
        }})]);
    }
}

#[tokio::test]
async fn completed_items_preserve_final_reasoning_and_tool_arguments() {
    let reasoning = json!({"type":"reasoning", "id":"rs_1", "summary":[], "encrypted_content":"final-ciphertext"});
    let call = json!({"type":"function_call", "id":"fc_1", "call_id":"call_1", "name":"lookup", "arguments":"{\"key\":1}"});
    let body = [
        json!({"type":"response.output_item.added", "output_index":0, "item":{
            "type":"reasoning", "id":"rs_1", "summary":[], "encrypted_content":"partial-ciphertext"
        }}),
        json!({"type":"response.output_item.done", "output_index":0, "item":reasoning.clone()}),
        json!({"type":"response.output_item.added", "output_index":1, "item":{
            "type":"function_call", "id":"fc_1", "call_id":"call_1", "name":"lookup", "arguments":""
        }}),
        json!({"type":"response.output_item.done", "output_index":1, "item":call}),
        json!({"type":"response.completed", "response":{"id":"resp_1", "status":"completed", "output":[]}}),
    ].iter().map(|event| format!("data: {event}\n\n")).collect::<String>();
    let events = collect_events(body).await;
    assert_eq!(events.last(), Some(&Ok(StreamEvent::MessageDone { response: ModelResponse {
        id:"resp_1".to_string(),
        content:vec![
            ResponseContent::ProviderReasoning {provider:"openai-responses".to_string(), payload:reasoning},
            ResponseContent::ToolUse {id:"call_1".to_string(), name:"lookup".to_string(), input:json!({"key":1})},
        ],
        stop_reason:Some(StopReason::EndTurn), usage:Usage::default(), metadata:ResponseMetadata::default(),
    }})));
}

#[tokio::test]
async fn truncated_function_arguments_cannot_become_empty_inputs() {
    let events = collect_events([
        json!({"type":"response.output_item.added", "output_index":0, "item":{
            "type":"function_call", "id":"fc_1", "call_id":"call_1", "name":"lookup", "arguments":""
        }}),
        json!({"type":"response.function_call_arguments.delta", "item_id":"fc_1", "delta":"{"}),
        json!({"type":"response.completed", "response":{"id":"resp_1", "status":"completed", "output":[]}}),
    ].iter().map(|event| format!("data: {event}\n\n")).collect()).await;
    assert!(events.last().is_some_and(Result::is_err));
    assert!(!events.iter().any(|event| matches!(event, Ok(StreamEvent::MessageDone { .. }))));
}

async fn collect_events(body: String) -> Vec<Result<StreamEvent, Value>> {
    let (provider, server) = spawn_provider(body).await;
    let events = timeout(Duration::from_secs(10), async {
        provider
            .completion_stream(minimal_request())
            .await
            .expect("HTTP 200 SSE stream")
            .map(|event| {
                event.map_err(|error| {
                    serde_json::to_value(
                        error
                            .downcast_ref::<ProviderError>()
                            .expect("typed provider error"),
                    )
                    .expect("serialize provider error")
                })
            })
            .collect::<Vec<_>>()
            .await
    })
    .await
    .expect("local SSE stream must terminate");
    server.await.expect("local SSE server completed");
    events
}

async fn spawn_provider(body: String) -> (OpenAIResponsesProvider, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local SSE server");
    let address = listener.local_addr().expect("local SSE address");
    let server = tokio::spawn(async move {
        timeout(Duration::from_secs(10), async move {
            let (mut socket, _) = listener.accept().await.expect("accept provider request");
            let mut request = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let count = socket
                    .read(&mut buffer)
                    .await
                    .expect("read provider request");
                assert_ne!(count, 0, "request closed before its body was received");
                request.extend_from_slice(&buffer[..count]);
                if let Some(header_end) =
                    request.windows(4).position(|window| window == b"\r\n\r\n")
                {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .filter_map(|line| line.split_once(':'))
                        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .map(|(_, value)| {
                            value
                                .trim()
                                .parse::<usize>()
                                .expect("request content length")
                        })
                        .expect("JSON request has a content length");
                    if request.len() >= header_end + 4 + content_length {
                        break;
                    }
                }
            }
            let content_length = body.len();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                 Content-Length: {content_length}\r\nConnection: close\r\n\r\n{body}"
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write HTTP 200 SSE response");
            socket.shutdown().await.expect("close SSE response");
        })
        .await
        .expect("local SSE server must terminate");
    });
    let http_options = ProviderHttpOptions::from_raw_with_no_proxy(
        /*proxy_url*/ None,
        Some("*".to_string()),
        /*headers*/ None,
    )
    .expect("explicit proxy bypass");
    let provider = OpenAIResponsesProvider::new(format!("http://{address}"))
        .with_http_options(http_options)
        .expect("local provider with no proxy");
    (provider, server)
}

fn minimal_request() -> ModelRequest {
    ModelRequest {
        model_slug: ModelProfileKey::Generic,
        model: "test-model".to_string(),
        system: None,
        messages: vec![RequestMessage {
            role: "user".to_string(),
            content: vec![RequestContent::Text {
                text: "Hello".to_string(),
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

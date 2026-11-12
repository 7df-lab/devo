use crate::dsml::DsmlToolCallHealer;
use devo_protocol::ModelRequest;
use devo_protocol::ReasoningEffort;
use devo_protocol::RequestContent;
use devo_protocol::RequestMessage;
use devo_protocol::SamplingControls;
use devo_protocol::ToolDefinition;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::build_request;
use super::parse_response;
use super::parse_stop_reason;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseExtra;
use devo_protocol::StopReason;

#[test]
fn build_request_includes_sampling_tools_and_thinking() {
    let request = ModelRequest {
        model_slug: devo_protocol::ModelProfileKey::Generic,
        model: "claude-sonnet-4-6".to_string(),
        system: Some("You are helpful.".to_string()),
        messages: vec![
            RequestMessage {
                role: "assistant".to_string(),
                content: vec![
                    RequestContent::Text {
                        text: "Calling tool".to_string(),
                    },
                    RequestContent::ToolUse {
                        id: "toolu_123".to_string(),
                        name: "get_weather".to_string(),
                        input: json!({"city": "Boston"}),
                    },
                ],
            },
            RequestMessage {
                role: "user".to_string(),
                content: vec![RequestContent::ToolResult {
                    tool_use_id: "toolu_123".to_string(),
                    content: "{\"temp\":72}".to_string(),
                    is_error: Some(false),
                }],
            },
        ],
        max_tokens: 1024,
        tools: Some(vec![ToolDefinition {
            name: "get_weather".to_string(),
            description: "Get weather by city".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": { "city": { "type": "string" } },
                "required": ["city"]
            }),
            output_schema: None,
        }]),
        hosted_tools: Vec::new(),
        sampling: SamplingControls {
            temperature: Some(0.2),
            top_p: Some(0.9),
            top_k: Some(32),
        },
        request_thinking: Some("medium".to_string()),
        reasoning_effort: None,
        extra_body: None,
    };

    let body = build_request(&request, /*stream*/ true).expect("request body");

    assert_eq!(body["model"], json!("claude-sonnet-4-6"));
    assert_eq!(body["max_tokens"], json!(1024));
    assert_eq!(body["stream"], json!(true));
    assert_eq!(body["system"], json!("You are helpful."));
    assert_eq!(body.get("temperature"), None);
    assert_eq!(body.get("top_p"), None);
    assert_eq!(body.get("top_k"), None);
    assert_eq!(body["thinking"]["type"], json!("adaptive"));
    assert_eq!(body["output_config"]["effort"], json!("medium"));
    assert_eq!(body["messages"][0]["role"], json!("assistant"));
    assert_eq!(body["messages"][0]["content"][1]["type"], json!("tool_use"));
    assert_eq!(
        body["messages"][1]["content"][0]["type"],
        json!("tool_result")
    );
    assert_eq!(body["tools"][0]["name"], json!("get_weather"));
}

#[test]
fn build_request_skips_unsigned_reasoning_blocks() {
    let request = ModelRequest {
        model_slug: devo_protocol::ModelProfileKey::Generic,
        model: "deepseek-v4-flash".to_string(),
        system: None,
        messages: vec![RequestMessage {
            role: "assistant".to_string(),
            content: vec![
                RequestContent::Reasoning {
                    text: "Need to inspect the file first.".to_string(),
                },
                RequestContent::Text {
                    text: "I'll read the README.".to_string(),
                },
                RequestContent::ToolUse {
                    id: "toolu_123".to_string(),
                    name: "read".to_string(),
                    input: json!({"path": "README.md"}),
                },
            ],
        }],
        max_tokens: 1024,
        tools: None,
        hosted_tools: Vec::new(),
        sampling: SamplingControls::default(),
        request_thinking: Some("enabled".to_string()),
        reasoning_effort: None,
        extra_body: None,
    };

    let body = build_request(&request, /*stream*/ true).expect("request body");

    assert_eq!(
        body["messages"][0]["content"],
        json!([
            {
                "type": "text",
                "text": "I'll read the README."
            },
            {
                "type": "tool_use",
                "id": "toolu_123",
                "name": "read",
                "input": { "path": "README.md" }
            }
        ])
    );
}

#[test]
fn build_request_omits_messages_with_no_anthropic_content() {
    let request = ModelRequest {
        model_slug: devo_protocol::ModelProfileKey::Generic,
        model: "claude-sonnet-4-6".to_string(),
        system: None,
        messages: vec![
            RequestMessage {
                role: "assistant".to_string(),
                content: vec![RequestContent::Reasoning {
                    text: "unsigned reasoning".to_string(),
                }],
            },
            RequestMessage {
                role: "user".to_string(),
                content: vec![RequestContent::Text {
                    text: "continue".to_string(),
                }],
            },
        ],
        max_tokens: 1024,
        tools: None,
        hosted_tools: Vec::new(),
        sampling: SamplingControls::default(),
        request_thinking: None,
        reasoning_effort: None,
        extra_body: None,
    };

    let body = build_request(&request, /*stream*/ false).expect("request body");

    assert_eq!(body["messages"].as_array().map(Vec::len), Some(1));
    assert_eq!(body["messages"][0]["role"], json!("user"));
    assert_eq!(body["messages"][0]["content"][0]["text"], json!("continue"));
}

#[test]
fn build_request_serializes_provider_reasoning_with_signature() {
    let request = ModelRequest {
        model_slug: devo_protocol::ModelProfileKey::Generic,
        model: "deepseek-v4-pro".to_string(),
        system: None,
        messages: vec![RequestMessage {
            role: "assistant".to_string(),
            content: vec![RequestContent::ProviderReasoning {
                provider: "anthropic".to_string(),
                payload: json!({
                    "type": "thinking",
                    "thinking": "Need to inspect the file first.",
                    "signature": "sig_123"
                }),
            }],
        }],
        max_tokens: 1024,
        tools: None,
        hosted_tools: Vec::new(),
        sampling: SamplingControls::default(),
        request_thinking: Some("enabled".to_string()),
        reasoning_effort: None,
        extra_body: None,
    };

    let body = build_request(&request, /*stream*/ true).expect("request body");

    assert_eq!(
        body["messages"][0]["content"],
        json!([{
            "type": "thinking",
            "thinking": "Need to inspect the file first.",
            "signature": "sig_123"
        }])
    );
}

#[test]
fn build_request_serializes_hosted_tool_use_blocks() {
    let request = ModelRequest {
        model_slug: devo_protocol::ModelProfileKey::Generic,
        model: "deepseek-v4-pro".to_string(),
        system: None,
        messages: vec![RequestMessage {
            role: "assistant".to_string(),
            content: vec![
                RequestContent::HostedToolUse {
                    id: "srvtool_1".to_string(),
                    name: "web_search".to_string(),
                    input: json!({"query": "Rust docs"}),
                    output: None,
                    status: None,
                },
                RequestContent::HostedToolUse {
                    id: "srvtool_1".to_string(),
                    name: "web_search".to_string(),
                    input: json!({"query": "Rust docs"}),
                    output: Some(json!([{
                        "title": "Rust documentation",
                        "url": "https://example.test/rust"
                    }])),
                    status: Some("completed".to_string()),
                },
            ],
        }],
        max_tokens: 1024,
        tools: None,
        hosted_tools: Vec::new(),
        sampling: SamplingControls::default(),
        request_thinking: None,
        reasoning_effort: None,
        extra_body: None,
    };

    let body = build_request(&request, /*stream*/ true).expect("request body");

    assert_eq!(
        body["messages"][0]["content"],
        json!([
            {
                "type": "server_tool_use",
                "id": "srvtool_1",
                "name": "web_search",
                "input": { "query": "Rust docs" }
            },
            {
                "type": "web_search_tool_result",
                "tool_use_id": "srvtool_1",
                "content": [{
                    "title": "Rust documentation",
                    "url": "https://example.test/rust"
                }]
            }
        ])
    );
}

#[test]
fn build_request_sends_disabled_thinking_without_output_config() {
    let mut request = ModelRequest {
        model_slug: devo_protocol::ModelProfileKey::Generic,
        model: "deepseek-v4-flash".to_string(),
        system: None,
        messages: vec![RequestMessage {
            role: "user".to_string(),
            content: vec![RequestContent::Text {
                text: "Reply with OK only.".to_string(),
            }],
        }],
        max_tokens: 1024,
        tools: None,
        hosted_tools: Vec::new(),
        sampling: SamplingControls::default(),
        request_thinking: Some("disabled".to_string()),
        reasoning_effort: None,
        extra_body: None,
    };

    let disabled = build_request(&request, /*stream*/ true).expect("request body");
    request.request_thinking = Some("bogus".to_string());
    let unknown = build_request(&request, /*stream*/ true).expect("request body");

    assert_eq!(disabled["thinking"]["type"], json!("disabled"));
    assert_eq!(disabled.get("output_config"), None);
    assert_eq!(unknown.get("thinking"), None);
    assert_eq!(unknown.get("output_config"), None);
}

#[test]
fn build_request_maps_max_thinking_to_output_config_effort() {
    let request = ModelRequest {
        model_slug: devo_protocol::ModelProfileKey::Generic,
        model: "deepseek-v4-pro".to_string(),
        system: None,
        messages: vec![RequestMessage {
            role: "user".to_string(),
            content: vec![RequestContent::Text {
                text: "Reply with OK only.".to_string(),
            }],
        }],
        max_tokens: 1024,
        tools: None,
        hosted_tools: Vec::new(),
        sampling: SamplingControls::default(),
        request_thinking: Some("max".to_string()),
        reasoning_effort: None,
        extra_body: None,
    };

    let body = build_request(&request, /*stream*/ true).expect("request body");

    assert_eq!(body["thinking"]["type"], json!("enabled"));
    assert_eq!(body["output_config"]["effort"], json!("max"));
}

#[test]
fn build_request_prefers_reasoning_effort_for_output_config_effort() {
    let request = ModelRequest {
        model_slug: devo_protocol::ModelProfileKey::Generic,
        model: "deepseek-v4-pro".to_string(),
        system: None,
        messages: vec![RequestMessage {
            role: "user".to_string(),
            content: vec![RequestContent::Text {
                text: "Reply with OK only.".to_string(),
            }],
        }],
        max_tokens: 1024,
        tools: None,
        hosted_tools: Vec::new(),
        sampling: SamplingControls::default(),
        request_thinking: Some("enabled".to_string()),
        reasoning_effort: Some(ReasoningEffort::Max),
        extra_body: None,
    };

    let body = build_request(&request, /*stream*/ true).expect("request body");

    assert_eq!(body["thinking"]["type"], json!("enabled"));
    assert_eq!(body["output_config"]["effort"], json!("max"));
}

#[test]
fn parse_response_extracts_text_tool_use_reasoning_and_usage() {
    let response = parse_response(
        json!({
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-6",
            "content": [
                {
                    "type": "thinking",
                    "thinking": "Need to call the weather tool first.",
                    "signature": "sig_123"
                },
                {
                    "type": "text",
                    "text": "Let me check that."
                },
                {
                    "type": "server_tool_use",
                    "id": "srvtool_1",
                    "name": "web_search",
                    "input": { "query": "Boston weather" },
                    "caller": { "type": "direct" }
                }
            ],
            "stop_reason": "tool_use",
            "usage": {
                "input_tokens": 11,
                "output_tokens": 7,
                "cache_creation_input_tokens": 3,
                "cache_read_input_tokens": 5,
                "service_tier": "standard",
                "inference_geo": "us"
            }
        }),
        &DsmlToolCallHealer::for_model("claude-sonnet-4-6"),
    )
    .expect("parse response");

    assert_eq!(response.id, "msg_123");
    assert_eq!(response.stop_reason, Some(StopReason::ToolUse));
    assert_eq!(response.usage.input_tokens, 19);
    assert_eq!(response.usage.output_tokens, 7);
    assert_eq!(response.usage.cache_creation_input_tokens, Some(3));
    assert_eq!(response.usage.cache_read_input_tokens, Some(5));
    assert_eq!(response.content.len(), 3);
    match &response.content[0] {
        ResponseContent::ProviderReasoning { provider, payload } => {
            assert_eq!(provider, "anthropic");
            assert_eq!(
                payload,
                &json!({
                    "type": "thinking",
                    "thinking": "Need to call the weather tool first.",
                    "signature": "sig_123"
                })
            );
        }
        other => panic!("expected provider reasoning block, got {other:?}"),
    }
    match &response.content[1] {
        ResponseContent::Text(text) => {
            assert_eq!(text, "Let me check that.");
        }
        other => panic!("expected text block, got {other:?}"),
    }
    match &response.content[2] {
        ResponseContent::HostedToolUse {
            id,
            name,
            input,
            output,
            status,
        } => {
            assert_eq!(id, "srvtool_1");
            assert_eq!(name, "web_search");
            assert_eq!(input, &json!({"query": "Boston weather"}));
            assert_eq!(output, &None);
            assert_eq!(status, &None);
        }
        other => panic!("expected hosted tool use block, got {other:?}"),
    }
    assert!(response.metadata.extras.iter().any(|extra| matches!(
        extra,
        ResponseExtra::ReasoningText { text }
        if text == "Need to call the weather tool first."
    )));
    assert!(response.metadata.extras.iter().any(|extra| matches!(
        extra,
        ResponseExtra::ProviderSpecific { provider, .. } if provider == "anthropic"
    )));
}

#[test]
fn parse_response_heals_deepseek_v4_dsml_text_tool_calls() {
    let response = parse_response(
        json!({
            "id": "msg_dsml",
            "type": "message",
            "role": "assistant",
            "model": "deepseek-v4-pro",
            "content": [
                {
                    "type": "text",
                    "text": "<｜｜DSML｜｜tool_calls>\n<｜｜DSML｜｜invoke name=\"web_search\">\n<｜｜DSML｜｜parameter name=\"query\" string=\"true\">electron-vite npm package 2026</｜｜DSML｜｜parameter>\n<｜｜DSML｜｜parameter name=\"limit\" string=\"false\">3</｜｜DSML｜｜parameter>\n</｜｜DSML｜｜invoke>\n</｜｜DSML｜｜tool_calls>"
                }
            ],
            "stop_reason": "tool_use",
            "usage": {
                "input_tokens": 4,
                "output_tokens": 2
            }
        }),
        &DsmlToolCallHealer::for_model("deepseek-v4-pro"),
    )
    .expect("parse response");

    assert_eq!(
        response.content,
        vec![ResponseContent::ToolUse {
            id: "dsml_0_0".to_string(),
            name: "web_search".to_string(),
            input: json!({
                "query": "electron-vite npm package 2026",
                "limit": 3
            }),
        }]
    );
}

#[test]
fn parse_response_extracts_web_search_tool_result_as_hosted_completion() {
    let response = parse_response(
        json!({
            "id": "msg_456",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-6",
            "content": [
                {
                    "type": "web_search_tool_result",
                    "tool_use_id": "srvtool_1",
                    "content": [
                        {
                            "type": "web_search_result",
                            "title": "Boston weather",
                            "url": "https://example.test/weather"
                        }
                    ],
                    "status": "completed"
                }
            ],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 4,
                "output_tokens": 2
            }
        }),
        &DsmlToolCallHealer::for_model("claude-sonnet-4-6"),
    )
    .expect("parse response");

    assert_eq!(response.content.len(), 1);
    match &response.content[0] {
        ResponseContent::HostedToolUse {
            id,
            name,
            input,
            output,
            status,
        } => {
            assert_eq!(id, "srvtool_1");
            assert_eq!(name, "web_search");
            assert_eq!(input, &json!({}));
            assert_eq!(
                output,
                &Some(json!([
                    {
                        "type": "web_search_result",
                        "title": "Boston weather",
                        "url": "https://example.test/weather"
                    }
                ]))
            );
            assert_eq!(status.as_deref(), Some("completed"));
        }
        other => panic!("expected hosted tool completion, got {other:?}"),
    }
}

#[test]
fn parse_response_carries_hosted_web_search_input_into_completion() {
    let response = parse_response(
        json!({
            "id": "msg_789",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-6",
            "content": [
                {
                    "type": "server_tool_use",
                    "id": "srvtool_1",
                    "name": "web_search",
                    "input": { "query": "DeepSeek official website" }
                },
                {
                    "type": "web_search_tool_result",
                    "tool_use_id": "srvtool_1",
                    "content": [
                        {
                            "type": "web_search_result",
                            "title": "DeepSeek",
                            "url": "https://www.deepseek.com/"
                        }
                    ],
                    "status": "completed"
                }
            ],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 4,
                "output_tokens": 2
            }
        }),
        &DsmlToolCallHealer::for_model("claude-sonnet-4-6"),
    )
    .expect("parse response");

    assert_eq!(
        response.content,
        vec![
            ResponseContent::HostedToolUse {
                id: "srvtool_1".to_string(),
                name: "web_search".to_string(),
                input: json!({"query": "DeepSeek official website"}),
                output: None,
                status: None,
            },
            ResponseContent::HostedToolUse {
                id: "srvtool_1".to_string(),
                name: "web_search".to_string(),
                input: json!({"query": "DeepSeek official website"}),
                output: Some(json!([
                    {
                        "type": "web_search_result",
                        "title": "DeepSeek",
                        "url": "https://www.deepseek.com/"
                    }
                ])),
                status: Some("completed".to_string()),
            },
        ]
    );
}

#[test]
fn parse_response_extracts_web_fetch_tool_result_as_hosted_completion() {
    let response = parse_response(
        json!({
            "id": "msg_fetch",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-6",
            "content": [
                {
                    "type": "server_tool_use",
                    "id": "srvtool_fetch",
                    "name": "web_fetch",
                    "input": { "url": "https://example.test/docs" }
                },
                {
                    "type": "web_fetch_tool_result",
                    "tool_use_id": "srvtool_fetch",
                    "content": {
                        "type": "web_fetch_result",
                        "url": "https://example.test/docs",
                        "title": "Docs"
                    },
                    "status": "completed"
                }
            ],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 4,
                "output_tokens": 2
            }
        }),
        &DsmlToolCallHealer::for_model("claude-sonnet-4-6"),
    )
    .expect("parse response");

    assert_eq!(
        response.content,
        vec![
            ResponseContent::HostedToolUse {
                id: "srvtool_fetch".to_string(),
                name: "web_fetch".to_string(),
                input: json!({"url": "https://example.test/docs"}),
                output: None,
                status: None,
            },
            ResponseContent::HostedToolUse {
                id: "srvtool_fetch".to_string(),
                name: "web_fetch".to_string(),
                input: json!({"url": "https://example.test/docs"}),
                output: Some(json!({
                    "type": "web_fetch_result",
                    "url": "https://example.test/docs",
                    "title": "Docs"
                })),
                status: Some("completed".to_string()),
            },
        ]
    );
}

#[test]
fn parse_stop_reason_matches_messages_contract() {
    assert_eq!(parse_stop_reason("end_turn"), StopReason::EndTurn);
    assert_eq!(parse_stop_reason("tool_use"), StopReason::ToolUse);
    assert_eq!(parse_stop_reason("max_tokens"), StopReason::MaxTokens);
    assert_eq!(parse_stop_reason("stop_sequence"), StopReason::StopSequence);
    assert_eq!(parse_stop_reason("pause_turn"), StopReason::PauseTurn);
    assert_eq!(parse_stop_reason("refusal"), StopReason::EndTurn);
}

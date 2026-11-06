use devo_protocol::{
    ModelRequest, RequestContent, RequestMessage, SamplingControls, ToolDefinition,
};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::response::parse_response;
use devo_protocol::{ResponseContent, ResponseExtra};

use crate::openai::responses::build_request;

#[test]
fn debug_request_body_includes_reasoning_and_tools() {
    let request = ModelRequest {
        model_slug: devo_protocol::ModelProfileKey::CatalogSlug("custom-model".to_string()),
        model: "custom-model".to_string(),
        system: Some("You are helpful.".to_string()),
        messages: vec![RequestMessage {
            role: "user".to_string(),
            content: vec![RequestContent::Text {
                text: "hi".to_string(),
            }],
        }],
        max_tokens: 256,
        tools: Some(vec![ToolDefinition {
            name: "get_weather".to_string(),
            description: "Get weather by city".to_string(),
            input_schema: json!({"type": "object"}),
            output_schema: None,
        }]),
        hosted_tools: Vec::new(),
        sampling: SamplingControls {
            temperature: Some(0.4),
            top_p: Some(0.7),
            top_k: Some(12),
        },
        request_thinking: Some("medium".to_string()),
        reasoning_effort: Some(devo_protocol::ReasoningEffort::Medium),
        extra_body: None,
    };

    let body = build_request(&request, true);

    assert_eq!(body["model"], json!("custom-model"));
    assert_eq!(body["stream"], json!(true));
    assert!(body.get("max_output_tokens").is_none());
    assert_eq!(body["temperature"], json!(0.4));
    assert_eq!(body["top_p"], json!(0.7));
    assert!(body.get("top_k").is_none());
    assert_eq!(body["tools"][0]["type"], json!("function"));
    assert_eq!(body["tools"][0]["name"], json!("get_weather"));
    assert!(body["tools"][0].get("function").is_none());
    // The system prompt rides the Responses `instructions` field; input
    // items never carry a system role (codex backends reject it).
    assert_eq!(body["instructions"], json!("You are helpful."));
    assert_eq!(body["input"][0]["role"], json!("user"));
    for item in body["input"].as_array().expect("input items") {
        assert_ne!(item["role"], json!("system"));
    }
}

#[test]
fn gpt5_responses_profile_strips_sampling_parameters() {
    let request = ModelRequest {
        model_slug: devo_protocol::ModelProfileKey::CatalogSlug("gpt-5.6-luna".to_string()),
        model: "gpt-5.6-luna".to_string(),
        system: Some("You are helpful.".to_string()),
        messages: vec![RequestMessage {
            role: "user".to_string(),
            content: vec![RequestContent::Text {
                text: "hi".to_string(),
            }],
        }],
        max_tokens: 256,
        tools: None,
        hosted_tools: Vec::new(),
        sampling: SamplingControls {
            temperature: Some(0.4),
            top_p: Some(0.7),
            top_k: Some(12),
        },
        request_thinking: Some("medium".to_string()),
        reasoning_effort: Some(devo_protocol::ReasoningEffort::Medium),
        extra_body: None,
    };

    let body = build_request(&request, true);

    assert!(body.get("temperature").is_none());
    assert!(body.get("top_p").is_none());
    assert!(body.get("top_k").is_none());
    assert!(body.get("max_output_tokens").is_none());
    assert_eq!(
        body["reasoning"],
        json!({ "effort": "medium", "summary": "auto" })
    );
    assert_eq!(body["store"], json!(false));
    assert_eq!(body["instructions"], json!("You are helpful."));
}

#[test]
fn build_request_omits_nested_reasoning_and_uses_output_text_for_assistant() {
    let request = ModelRequest {
        model_slug: devo_protocol::ModelProfileKey::CatalogSlug("deepseek-v4-flash".to_string()),
        model: "deepseek-v4-flash".to_string(),
        system: None,
        messages: vec![
            RequestMessage {
                role: "user".to_string(),
                content: vec![RequestContent::Text {
                    text: "hi".to_string(),
                }],
            },
            RequestMessage {
                role: "assistant".to_string(),
                content: vec![
                    RequestContent::Reasoning {
                        text: "secret thoughts".to_string(),
                    },
                    RequestContent::Text {
                        text: "hello".to_string(),
                    },
                ],
            },
        ],
        max_tokens: 64,
        tools: None,
        hosted_tools: Vec::new(),
        sampling: SamplingControls {
            temperature: None,
            top_p: None,
            top_k: None,
        },
        request_thinking: None,
        reasoning_effort: None,
        extra_body: None,
    };

    let body = build_request(&request, false);
    let input = body["input"].as_array().expect("input array");
    assert_eq!(input.len(), 2);
    assert_eq!(input[0]["content"][0]["type"], json!("input_text"));
    assert_eq!(input[1]["role"], json!("assistant"));
    assert_eq!(input[1]["content"].as_array().map(|c| c.len()), Some(1));
    assert_eq!(input[1]["content"][0]["type"], json!("output_text"));
    assert_eq!(input[1]["content"][0]["text"], json!("hello"));
    let serialized = body.to_string();
    assert!(
        !serialized.contains("\"type\":\"reasoning\""),
        "nested reasoning content must not appear in Responses input: {serialized}"
    );
}

#[test]
fn build_request_omits_unsupported_hosted_tool_history() {
    let request = ModelRequest {
        model_slug: devo_protocol::ModelProfileKey::CatalogSlug("gpt-5.4".to_string()),
        model: "gpt-5.4".to_string(),
        system: None,
        messages: vec![
            RequestMessage {
                role: "user".to_string(),
                content: vec![RequestContent::Text {
                    text: "before".to_string(),
                }],
            },
            RequestMessage {
                role: "assistant".to_string(),
                content: vec![
                    RequestContent::HostedToolUse {
                        id: "hosted_ws_1".to_string(),
                        name: "web_search".to_string(),
                        input: json!({"query": "Rust docs"}),
                        output: None,
                        status: None,
                    },
                    RequestContent::HostedToolUse {
                        id: "hosted_ws_1".to_string(),
                        name: "web_search".to_string(),
                        input: json!({"query": "Rust docs"}),
                        output: Some(json!([{
                            "title": "Rust documentation",
                            "url": "https://example.test/rust"
                        }])),
                        status: Some("completed".to_string()),
                    },
                ],
            },
            RequestMessage {
                role: "user".to_string(),
                content: vec![RequestContent::Text {
                    text: "after".to_string(),
                }],
            },
        ],
        max_tokens: 256,
        tools: None,
        hosted_tools: Vec::new(),
        sampling: SamplingControls::default(),
        request_thinking: None,
        reasoning_effort: None,
        extra_body: None,
    };

    let body = build_request(&request, false);

    assert_eq!(body["input"].as_array().map(Vec::len), Some(4));
    assert_eq!(body["input"][0]["role"], json!("user"));
    assert_eq!(body["input"][1]["type"], json!("web_search_call"));
    assert_eq!(body["input"][2]["type"], json!("web_search_call"));
    assert_eq!(body["input"][3]["role"], json!("user"));
    let serialized = serde_json::to_string(&body).expect("serialize request body");
    assert!(!serialized.contains("hosted_tool_use"));
    assert!(!serialized.contains("\"type\":\"tool_call\""));
}

#[test]
fn parse_response_extracts_text_and_tool_use() {
    let response = parse_response(json!({
        "id": "resp_123",
        "status": "completed",
        "output": [
            {
                "type": "message",
                "content": [
                    {"type": "output_text", "text": "Hello"},
                    {
                        "type": "function_call",
                        "id": "call_1",
                        "name": "get_weather",
                        "arguments": {"city": "Boston"}
                    }
                ]
            }
        ],
        "usage": {
            "input_tokens": 10,
            "output_tokens": 5,
            "total_tokens": 21,
            "output_tokens_details": {
                "reasoning_tokens": 6
            }
        }
    }))
    .expect("parse response");

    assert_eq!(response.id, "resp_123");
    assert_eq!(response.content.len(), 2);
    assert_eq!(response.usage.reasoning_output_tokens, Some(6));
    assert_eq!(response.usage.total_tokens, Some(21));
    assert!(matches!(response.content[0], ResponseContent::Text(_)));
    assert!(matches!(
        response.content[1],
        ResponseContent::ToolUse { .. }
    ));
}

#[test]
fn parse_response_preserves_cached_input_tokens() {
    let response = parse_response(json!({
        "id": "resp_cache",
        "status": "completed",
        "output": [],
        "usage": {
            "input_tokens": 100,
            "input_tokens_details": { "cached_tokens": 73 },
            "output_tokens": 4,
            "total_tokens": 104
        }
    }))
    .expect("parse response");

    assert_eq!(response.usage.input_tokens, 100);
    assert_eq!(response.usage.cache_read_input_tokens, Some(73));
    assert_eq!(response.usage.total_tokens, Some(104));
}

#[test]
fn parse_response_falls_back_to_prompt_tokens_cache_details() {
    let response = parse_response(json!({
        "id": "resp_prompt_cache",
        "status": "completed",
        "output": [],
        "usage": {
            "prompt_tokens": 20,
            "input_tokens_details": {},
            "prompt_tokens_details": { "cached_tokens": 12 },
            "completion_tokens": 2,
            "total_tokens": 22
        }
    }))
    .expect("parse response");

    assert_eq!(response.usage.cache_read_input_tokens, Some(12));
}

#[test]
fn parse_response_parses_string_tool_arguments_as_json() {
    let response = parse_response(json!({
        "id": "resp_args",
        "status": "completed",
        "output": [
            {
                "type": "function_call",
                "call_id": "call_1",
                "name": "weather",
                "arguments": "{\"city\":\"Boston\"}"
            }
        ]
    }))
    .expect("parse response");

    assert_eq!(
        response.content,
        vec![ResponseContent::ToolUse {
            id: "call_1".to_string(),
            name: "weather".to_string(),
            input: json!({"city": "Boston"}),
        }]
    );
}

#[test]
fn parse_response_extracts_web_search_call_as_hosted_tool_use() {
    let response = parse_response(json!({
        "id": "resp_789",
        "status": "completed",
        "output": [
            {
                "type": "web_search_call",
                "id": "ws_1",
                "status": "completed",
                "action": {
                    "type": "search",
                    "query": "Rust async docs"
                },
                "results": [
                    {
                        "title": "Async Programming in Rust",
                        "url": "https://example.test/rust-async"
                    }
                ]
            }
        ],
        "usage": {
            "input_tokens": 10,
            "output_tokens": 5
        }
    }))
    .expect("parse response");

    assert_eq!(
        response.content,
        vec![ResponseContent::HostedToolUse {
            id: "ws_1".to_string(),
            name: "web_search".to_string(),
            input: json!({"query": "Rust async docs"}),
            output: Some(json!([
                {
                    "title": "Async Programming in Rust",
                    "url": "https://example.test/rust-async"
                }
            ])),
            status: Some("completed".to_string()),
        }]
    );
}

#[test]
fn parse_response_extracts_web_fetch_call_as_hosted_tool_use() {
    let response = parse_response(json!({
        "id": "resp_fetch",
        "status": "completed",
        "output": [
            {
                "type": "web_fetch_call",
                "id": "wf_1",
                "status": "completed",
                "action": {
                    "type": "fetch",
                    "url": "https://example.test/docs"
                },
                "output": {
                    "title": "Docs",
                    "url": "https://example.test/docs"
                }
            }
        ],
        "usage": {
            "input_tokens": 10,
            "output_tokens": 5
        }
    }))
    .expect("parse response");

    assert_eq!(
        response.content,
        vec![ResponseContent::HostedToolUse {
            id: "wf_1".to_string(),
            name: "web_fetch".to_string(),
            input: json!({"url": "https://example.test/docs"}),
            output: Some(json!({
                "title": "Docs",
                "url": "https://example.test/docs"
            })),
            status: Some("completed".to_string()),
        }]
    );
}

#[test]
fn parse_response_preserves_reasoning_text_as_metadata() {
    let response = parse_response(json!({
        "id": "resp_456",
        "status": "completed",
        "output": [
            {
                "type": "message",
                "content": [
                    {"type": "output_text", "text": "final"}
                ],
                "reasoning_content": "internal reasoning"
            }
        ],
        "usage": {
            "input_tokens": 3,
            "output_tokens": 1
        }
    }))
    .expect("parse response");

    assert_eq!(response.metadata.extras.len(), 1);
    assert!(matches!(
        response.metadata.extras[0],
        ResponseExtra::ReasoningText { .. }
    ));
}

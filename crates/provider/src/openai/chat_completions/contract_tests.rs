use devo_protocol::{
    ModelProfileKey, ModelRequest, ReasoningEffort, RequestContent, RequestMessage,
    ResponseContent, ResponseExtra, SamplingControls, ToolDefinition,
};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::{build_request, parse_response};
use crate::dsml::DsmlToolCallHealer;
use crate::openai::shared::{responses_tool_definitions, tool_definitions};

fn request(profile: ModelProfileKey) -> ModelRequest {
    ModelRequest {
        model_slug: profile,
        model: "provider-deployment-name".to_string(),
        system: None,
        messages: vec![RequestMessage {
            role: "user".to_string(),
            content: vec![RequestContent::Text {
                text: "Hi".to_string(),
            }],
        }],
        max_tokens: 128,
        tools: None,
        hosted_tools: Vec::new(),
        sampling: SamplingControls::default(),
        request_thinking: None,
        reasoning_effort: None,
        extra_body: None,
    }
}

#[test]
fn official_reasoning_models_use_completion_token_limit() {
    for slug in [
        "openai/o1",
        "o3-mini",
        "o4-mini-2025-04-16",
        "openai/gpt-5",
        "gpt-5.4",
    ] {
        let body = build_request(
            &request(ModelProfileKey::CatalogSlug(slug.to_string())),
            false,
        );
        assert_eq!(
            body,
            json!({
                "model": "provider-deployment-name",
                "messages": [{ "role": "user", "content": "Hi" }],
                "max_completion_tokens": 128,
                "stream": false,
            })
        );
    }
}

#[test]
fn generic_and_compatible_models_keep_legacy_token_limit() {
    for profile in [
        ModelProfileKey::Generic,
        ModelProfileKey::CatalogSlug("vendor/llama".to_string()),
    ] {
        assert_eq!(
            build_request(&request(profile), false),
            json!({
                "model": "provider-deployment-name",
                "messages": [{ "role": "user", "content": "Hi" }],
                "max_tokens": 128,
                "stream": false,
            })
        );
    }
}

#[test]
fn reasoning_sampling_controls_follow_model_support() {
    for slug in ["o3", "gpt-5", "gpt-5.4"] {
        let mut request = request(ModelProfileKey::CatalogSlug(slug.to_string()));
        request.reasoning_effort = Some(ReasoningEffort::High);
        request.sampling = SamplingControls {
            temperature: Some(0.3),
            top_p: Some(0.9),
            top_k: None,
        };
        assert_eq!(
            build_request(&request, false),
            json!({
                "model": "provider-deployment-name",
                "messages": [{ "role": "user", "content": "Hi" }],
                "max_completion_tokens": 128,
                "reasoning_effort": "high",
                "stream": false,
            })
        );
    }
    let mut request = request(ModelProfileKey::CatalogSlug("gpt-5.4".to_string()));
    request.reasoning_effort = Some(ReasoningEffort::None);
    request.sampling.temperature = Some(0.3);
    assert_eq!(
        build_request(&request, false),
        json!({
            "model": "provider-deployment-name",
            "messages": [{ "role": "user", "content": "Hi" }],
            "max_completion_tokens": 128,
            "reasoning_effort": "none",
            "temperature": 0.3,
            "stream": false,
        })
    );
}

#[test]
fn gpt55_default_reasoning_does_not_send_sampling_controls() {
    let mut request = request(ModelProfileKey::CatalogSlug("gpt-5.5".to_string()));
    request.sampling.temperature = Some(0.3);
    assert_eq!(
        build_request(&request, false),
        json!({
            "model": "provider-deployment-name",
            "messages": [{ "role": "user", "content": "Hi" }],
            "max_completion_tokens": 128,
            "stream": false,
        })
    );
}

#[test]
fn explicit_limit_override_does_not_send_both_dialects() {
    let mut request = request(ModelProfileKey::Generic);
    request.extra_body = Some(json!({ "max_completion_tokens": 256 }));
    assert_eq!(
        build_request(&request, false),
        json!({
            "model": "provider-deployment-name",
            "messages": [{ "role": "user", "content": "Hi" }],
            "max_completion_tokens": 256,
            "stream": false,
        })
    );
    request.model_slug = ModelProfileKey::CatalogSlug("o3".to_string());
    request.extra_body = Some(json!({ "max_tokens": 512 }));
    assert_eq!(
        build_request(&request, false),
        json!({
            "model": "provider-deployment-name",
            "messages": [{ "role": "user", "content": "Hi" }],
            "max_tokens": 512,
            "stream": false,
        })
    );
}

#[test]
fn nullable_logprob_arrays_and_refusal_are_preserved() {
    let response = parse_response(
        json!({
            "id": "chatcmpl-refusal",
            "choices": [{
                "index": 0,
                "message": { "content": null, "refusal": "I cannot help with that." },
                "finish_reason": "stop",
                "logprobs": { "content": null, "refusal": null }
            }]
        }),
        &DsmlToolCallHealer::for_model("gpt-4o"),
    )
    .expect("valid nullable logprobs");
    assert_eq!(
        response.content,
        vec![ResponseContent::Text(
            "I cannot help with that.".to_string()
        )]
    );
    assert_eq!(
        response.metadata.extras,
        vec![ResponseExtra::ProviderSpecific {
            provider: "openai".to_string(),
            payload: json!({ "choices": [{
            "index": 0,
            "logprobs": { "content": [], "refusal": [] },
            "message": { "refusal": "I cannot help with that." }
        }] }),
        }]
    );
}

#[test]
fn malformed_function_arguments_do_not_become_empty_arguments() {
    let error = parse_response(
        json!({
            "id": "chatcmpl-tool",
            "choices": [{
                "message": { "tool_calls": [{
                    "id": "call_1", "type": "function",
                    "function": { "name": "read", "arguments": "{\"path\":" }
                }] },
                "finish_reason": "tool_calls"
            }]
        }),
        &DsmlToolCallHealer::for_model("gpt-4o"),
    )
    .expect_err("malformed arguments must fail");
    assert!(error.to_string().contains("invalid JSON"));
}

#[test]
fn function_tool_definitions_do_not_send_unsupported_output_schema() {
    let tools = vec![ToolDefinition {
        name: "read".to_string(),
        description: "Read a file".to_string(),
        input_schema: json!({ "type": "object", "properties": {} }),
        output_schema: Some(json!({ "type": "string" })),
    }];
    let definition = json!({
        "name": "read", "description": "Read a file",
        "parameters": { "type": "object", "properties": {} }
    });
    assert_eq!(
        tool_definitions(&tools),
        json!([{ "type": "function", "function": definition }])
    );
    let mut flat = definition;
    flat["type"] = Value::String("function".to_string());
    flat["output_schema"] = json!({ "type": "string" });
    assert_eq!(responses_tool_definitions(&tools), json!([flat]));
}

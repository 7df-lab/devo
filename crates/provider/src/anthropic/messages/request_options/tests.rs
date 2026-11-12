use super::super::build_request;
use devo_protocol::{ModelProfileKey, ModelRequest, ReasoningEffort, SamplingControls};
use pretty_assertions::assert_eq;
use serde_json::json;

fn request(model: &str) -> ModelRequest {
    ModelRequest {
        model_slug: ModelProfileKey::Generic,
        model: model.to_string(),
        system: None,
        messages: Vec::new(),
        max_tokens: 16000,
        tools: None,
        hosted_tools: Vec::new(),
        sampling: SamplingControls::default(),
        request_thinking: Some("high".to_string()),
        reasoning_effort: None,
        extra_body: None,
    }
}

#[test]
fn adaptive_and_manual_thinking_follow_model_contracts() {
    for (model, thinking, output) in [
        (
            "claude-sonnet-4-6",
            json!({"type":"adaptive"}),
            Some(json!({"effort":"high"})),
        ),
        (
            "claude-opus-4-7",
            json!({"type":"adaptive"}),
            Some(json!({"effort":"high"})),
        ),
        (
            "claude-sonnet-5",
            json!({"type":"adaptive"}),
            Some(json!({"effort":"high"})),
        ),
        (
            "claude-opus-4-5-20251101",
            json!({"type":"enabled","budget_tokens":8192}),
            Some(json!({"effort":"high"})),
        ),
        (
            "claude-haiku-4-5-20251001",
            json!({"type":"enabled","budget_tokens":8192}),
            None,
        ),
        (
            "claude-sonnet-4-20250514",
            json!({"type":"enabled","budget_tokens":8192}),
            None,
        ),
        (
            "claude-3-7-sonnet-20250219",
            json!({"type":"enabled","budget_tokens":8192}),
            None,
        ),
        (
            "deepseek-v4-pro",
            json!({"type":"enabled"}),
            Some(json!({"effort":"high"})),
        ),
    ] {
        let body = build_request(&request(model), /*stream*/ false).expect("request body");
        assert_eq!(
            (body["thinking"].clone(), body.get("output_config").cloned()),
            (thinking, output),
            "{model}"
        );
    }
}

#[test]
fn legacy_budget_leaves_output_room_and_rejects_impossible_limits() {
    let mut request = request("claude-haiku-4-5");
    request.max_tokens = 2048;
    assert_eq!(
        build_request(&request, /*stream*/ false).unwrap()["thinking"],
        json!({"type":"enabled", "budget_tokens":2047})
    );
    request.max_tokens = 1024;
    assert!(build_request(&request, /*stream*/ false).is_err());
}

#[test]
fn explicit_extra_body_can_override_generated_thinking_defaults() {
    let mut request = request("claude-sonnet-4-6");
    request.extra_body = Some(json!({"thinking":{"type":"enabled","budget_tokens":4096}}));
    assert_eq!(
        build_request(&request, /*stream*/ false).unwrap()["thinking"],
        json!({"type":"enabled", "budget_tokens":4096})
    );
}

#[test]
fn effort_without_thinking_is_not_silently_discarded_and_xhigh_is_supported_only_where_available() {
    for (model, expected) in [
        ("claude-sonnet-4-6", "high"),
        ("claude-opus-4-7", "xhigh"),
        ("claude-opus-4-5", "high"),
    ] {
        let mut request = request(model);
        request.request_thinking = None;
        request.reasoning_effort = Some(ReasoningEffort::XHigh);
        let body = build_request(&request, /*stream*/ false).unwrap();
        assert_eq!(
            (body.get("thinking"), body["output_config"].clone()),
            (None, json!({"effort": expected}))
        );
    }
}

#[test]
fn claude_thinking_removes_incompatible_sampling_without_affecting_compatible_backends() {
    for (model, expected) in [
        ("claude-sonnet-4-6", json!({"top_p":0.99})),
        ("claude-opus-4-7", json!({})),
        (
            "deepseek-v4-pro",
            json!({"temperature":0.2,"top_p":0.99,"top_k":32}),
        ),
    ] {
        let mut request = request(model);
        request.sampling = SamplingControls {
            temperature: Some(0.2),
            top_p: Some(0.99),
            top_k: Some(32),
        };
        let mut body = build_request(&request, /*stream*/ false).unwrap();
        body.as_object_mut()
            .unwrap()
            .retain(|key, _| ["temperature", "top_p", "top_k"].contains(&key.as_str()));
        assert_eq!(body, expected);
    }
}

#[test]
fn final_overrides_are_respected_before_generating_manual_budgets() {
    let mut request = request("claude-opus-4-5");
    request.max_tokens = 512;
    request.extra_body = Some(json!({"thinking":{"type":"disabled"}}));
    let body = build_request(&request, /*stream*/ false).unwrap();
    assert_eq!(body["thinking"], json!({"type":"disabled"}));
    request.extra_body = Some(json!({"max_tokens":4096}));
    let body = build_request(&request, /*stream*/ false).unwrap();
    assert_eq!(
        body["thinking"],
        json!({"type":"enabled","budget_tokens":4095})
    );
}

#[test]
fn disabled_and_between_tools_do_not_gain_invalid_effort() {
    for model in ["claude-opus-5", "claude-haiku-5-5"] {
        let mut request = request(model);
        request.request_thinking = Some("disabled".to_string());
        request.reasoning_effort = Some(ReasoningEffort::XHigh);
        let body = build_request(&request, /*stream*/ false).unwrap();
        assert_eq!(body.get("output_config"), None);
    }
    let mut request = request("claude-sonnet-5-5");
    request.request_thinking = Some("between_tools".to_string());
    request.reasoning_effort = Some(ReasoningEffort::Max);
    let body = build_request(&request, /*stream*/ false).unwrap();
    assert_eq!(body["output_config"], json!({"effort":"high"}));
}

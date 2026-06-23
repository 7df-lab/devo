use serde::{Deserialize, Serialize};

/// A content block in the model's response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ResponseContent {
    Text(String),
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    HostedToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<String>,
    },
    ProviderReasoning {
        provider: String,
        payload: serde_json::Value,
    },
}

/// Provider/model token usage statistics.
///
/// `output_tokens` is the provider-mapped primary output count.
/// `reasoning_output_tokens` is an optional breakdown and must not be added
/// again when deriving totals.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Number of tokens in the prompt.
    pub input_tokens: usize,
    /// Number of tokens in the generated completion.
    pub output_tokens: usize,
    /// The number of input tokens used to create the cache entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<usize>,
    /// The number of input tokens read from the cache.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<usize>,
    /// Optional provider-reported reasoning output token breakdown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_output_tokens: Option<usize>,
    /// Optional provider-reported total token count.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<usize>,
}

impl Usage {
    pub fn derived_total_tokens(&self) -> usize {
        self.input_tokens.saturating_add(self.output_tokens)
    }

    pub fn display_total_tokens(&self) -> usize {
        self.total_tokens
            .unwrap_or_else(|| self.derived_total_tokens())
    }
}

/// Why the model stopped generating.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    StopSequence,
}

/// Optional provider-specific response data preserved alongside the shared IR.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ResponseExtra {
    /// Reasoning text or reasoning summary surfaced by a provider.
    ReasoningText { text: String },
    /// Structured provider-specific payload that does not map into the shared IR.
    ProviderSpecific {
        provider: String,
        payload: serde_json::Value,
    },
}

/// Additional response metadata preserved by adapters.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResponseMetadata {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extras: Vec<ResponseExtra>,
}

/// Complete model response (non-streaming).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelResponse {
    pub id: String,
    pub content: Vec<ResponseContent>,
    pub stop_reason: Option<StopReason>,
    pub usage: Usage,
    #[serde(default)]
    pub metadata: ResponseMetadata,
}

/// Incremental events emitted during streaming.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StreamEvent {
    /// Start of a new text block.
    TextStart { index: usize },
    /// Incremental text delta.
    TextDelta { index: usize, text: String },
    /// Start of a new reasoning block.
    ReasoningStart { index: usize },
    /// Incremental reasoning delta.
    ReasoningDelta { index: usize, text: String },
    /// End of a reasoning block.
    ReasoningDone { index: usize },
    /// A tool call started.
    ToolCallStart {
        index: usize,
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// A provider-hosted tool call started. These calls are surfaced for
    /// client visibility, but must not be routed to local tool execution.
    HostedToolCallStart {
        index: usize,
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// A provider-hosted tool call completed.
    HostedToolCallDone {
        index: usize,
        id: String,
        name: String,
        input: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<String>,
    },
    /// Incremental JSON delta for tool input.
    ToolCallInputDelta { index: usize, partial_json: String },
    /// Usage update mid-stream.
    UsageDelta(Usage),
    /// The full message is complete.
    MessageDone { response: ModelResponse },
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::{
        ModelResponse, ResponseContent, ResponseExtra, ResponseMetadata, StopReason, StreamEvent,
        Usage,
    };

    #[test]
    fn usage_default() {
        let usage = Usage::default();
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
        assert_eq!(usage.cache_creation_input_tokens, None);
        assert_eq!(usage.cache_read_input_tokens, None);
        assert_eq!(usage.reasoning_output_tokens, None);
        assert_eq!(usage.total_tokens, None);
    }

    #[test]
    fn usage_totals_follow_conservative_accounting() {
        let usage = Usage {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
            reasoning_output_tokens: Some(25),
            total_tokens: None,
        };

        assert_eq!(usage.derived_total_tokens(), 150);
        assert_eq!(usage.display_total_tokens(), 150);

        let provider_total = Usage {
            total_tokens: Some(175),
            ..usage
        };
        assert_eq!(provider_total.derived_total_tokens(), 150);
        assert_eq!(provider_total.display_total_tokens(), 175);
    }

    #[test]
    fn usage_serde_skips_none_optional_usage_fields() {
        let usage = Usage {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
            reasoning_output_tokens: None,
            total_tokens: None,
        };
        let json = serde_json::to_string(&usage).expect("serialize usage");
        assert!(!json.contains("cache_creation"));
        assert!(!json.contains("cache_read"));
        assert!(!json.contains("reasoning_output"));
        assert!(!json.contains("total_tokens"));
    }

    #[test]
    fn usage_serde_defaults_missing_optional_usage_fields() {
        let usage: Usage = serde_json::from_value(json!({
            "input_tokens": 100,
            "output_tokens": 50
        }))
        .expect("deserialize legacy usage");

        assert_eq!(
            usage,
            Usage {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                reasoning_output_tokens: None,
                total_tokens: None,
            }
        );
    }

    #[test]
    fn stop_reason_serde() {
        for reason in [
            StopReason::EndTurn,
            StopReason::ToolUse,
            StopReason::MaxTokens,
            StopReason::StopSequence,
        ] {
            let json = serde_json::to_string(&reason).expect("serialize stop reason");
            let deserialized: StopReason =
                serde_json::from_str(&json).expect("deserialize stop reason");
            assert_eq!(deserialized, reason);
        }
    }

    #[test]
    fn model_response_serde() {
        let resp = ModelResponse {
            id: "msg-123".into(),
            content: vec![ResponseContent::Text("hello".into())],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                reasoning_output_tokens: None,
                total_tokens: None,
            },
            metadata: ResponseMetadata::default(),
        };
        let json = serde_json::to_string(&resp).expect("serialize response");
        let deserialized: ModelResponse =
            serde_json::from_str(&json).expect("deserialize response");
        assert_eq!(deserialized.id, "msg-123");
        assert_eq!(deserialized.content.len(), 1);
        assert_eq!(deserialized.stop_reason, Some(StopReason::EndTurn));
    }

    #[test]
    fn response_content_tool_use_serde() {
        let content = ResponseContent::ToolUse {
            id: "tu-1".into(),
            name: "bash".into(),
            input: json!({"cmd": "ls"}),
        };
        let json = serde_json::to_string(&content).expect("serialize content");
        let deserialized: ResponseContent =
            serde_json::from_str(&json).expect("deserialize content");
        match deserialized {
            ResponseContent::ToolUse { id, name, input } => {
                assert_eq!(id, "tu-1");
                assert_eq!(name, "bash");
                assert_eq!(input, json!({"cmd": "ls"}));
            }
            _ => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn response_content_hosted_tool_use_serde() {
        let content = ResponseContent::HostedToolUse {
            id: "ws-1".into(),
            name: "web_search".into(),
            input: json!({"query": "rust"}),
            output: Some(json!({"results": ["Rust docs"]})),
            status: Some("completed".into()),
        };
        let json = serde_json::to_string(&content).expect("serialize content");
        let deserialized: ResponseContent =
            serde_json::from_str(&json).expect("deserialize content");
        assert_eq!(deserialized, content);
    }

    #[test]
    fn response_extra_reasoning_text_roundtrip() {
        let extra = ResponseExtra::ReasoningText {
            text: "internal reasoning".into(),
        };
        let json = serde_json::to_string(&extra).expect("serialize response extra");
        let deserialized: ResponseExtra =
            serde_json::from_str(&json).expect("deserialize response extra");
        assert_eq!(deserialized, extra);
    }

    #[test]
    fn stream_event_tool_call_roundtrip() {
        let event = StreamEvent::ToolCallStart {
            index: 1,
            id: "call_123".into(),
            name: "get_weather".into(),
            input: json!({}),
        };
        let json = serde_json::to_string(&event).expect("serialize stream event");
        let deserialized: StreamEvent =
            serde_json::from_str(&json).expect("deserialize stream event");
        assert_eq!(deserialized, event);
    }

    #[test]
    fn stream_event_hosted_tool_call_roundtrip() {
        let event = StreamEvent::HostedToolCallDone {
            index: 1,
            id: "ws_123".into(),
            name: "web_search".into(),
            input: json!({"query": "rust"}),
            output: Some(json!({"results": ["Rust docs"]})),
            status: Some("completed".into()),
        };
        let json = serde_json::to_string(&event).expect("serialize stream event");
        let deserialized: StreamEvent =
            serde_json::from_str(&json).expect("deserialize stream event");
        assert_eq!(deserialized, event);
    }
}

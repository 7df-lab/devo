use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

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
    /// In-process provenance for a sample emitted by an explicit terminal
    /// provider event (including failures). Never accepted from serialized data.
    #[serde(skip)]
    pub terminal_sample: bool,
    /// Missing counters. `None` means a legacy sample with unknown coverage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unreported_fields: Option<Vec<crate::native::usage::UsageField>>,
    /// Whether a provider explicitly supplied token counters, including zero.
    /// Defaults and absent/null/empty provider usage are unreported. Legacy
    /// samples with nonzero or optional counters remain recognized by
    /// `is_reported` even when this field was not stored.
    #[serde(default)]
    pub reported: bool,
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
    pub fn into_terminal_sample(mut self) -> Self {
        self.terminal_sample = true;
        self
    }

    /// A zero in a progress sample can be a placeholder. Explicit terminal
    /// zero and any meaningful partial counters remain valid measurements.
    pub fn is_trusted_stream_sample(&self) -> bool {
        self.is_reported()
            && (self.terminal_sample
                || self.input_tokens > 0
                || self.output_tokens > 0
                || self.total_tokens.unwrap_or(0) > 0
                || self.cache_read_input_tokens.unwrap_or(0) > 0
                || self.cache_creation_input_tokens.unwrap_or(0) > 0
                || self.reasoning_output_tokens.unwrap_or(0) > 0)
    }

    /// Distinguishes an actual reported zero from a missing usage sample.
    /// Nonzero and optional legacy counters retain their previous meaning.
    pub fn is_reported(&self) -> bool {
        self.reported
            || self.input_tokens > 0
            || self.output_tokens > 0
            || self.cache_creation_input_tokens.is_some()
            || self.cache_read_input_tokens.is_some()
            || self.reasoning_output_tokens.is_some()
            || self.total_tokens.is_some()
    }

    /// Record counter presence after a provider has normalized its fields.
    /// Primary values are used only for their presence; normalized input may
    /// already include cache counts (Anthropic), so it is not replaced here.
    pub fn with_field_coverage(
        mut self,
        input_tokens: Option<usize>,
        output_tokens: Option<usize>,
    ) -> Self {
        use crate::native::usage::UsageField;
        if self.is_reported() {
            self.unreported_fields = Some(
                UsageField::ALL
                    .into_iter()
                    .filter(|field| match field {
                        UsageField::InputTokens => input_tokens.is_none(),
                        UsageField::OutputTokens => output_tokens.is_none(),
                        UsageField::ReasoningTokens => self.reasoning_output_tokens.is_none(),
                        UsageField::CacheReadInputTokens => self.cache_read_input_tokens.is_none(),
                        UsageField::CacheCreationInputTokens => {
                            self.cache_creation_input_tokens.is_none()
                        }
                        UsageField::TotalTokens => {
                            self.total_tokens.is_none()
                                && (input_tokens.is_none() || output_tokens.is_none())
                        }
                    })
                    .collect(),
            );
        }
        self
    }

    pub fn derived_total_tokens(&self) -> usize {
        self.input_tokens.saturating_add(self.output_tokens)
    }

    pub fn display_total_tokens(&self) -> usize {
        self.total_tokens
            .unwrap_or_else(|| self.derived_total_tokens())
    }
}

/// Why the model stopped generating.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    StopSequence,
    /// Server-side tool processing paused and requires replaying the assistant turn.
    PauseTurn,
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
    #[test]
    fn terminal_sample_provenance_is_internal_and_preserves_real_zero() {
        let zero = super::Usage {
            reported: true,
            ..super::Usage::default()
        };
        assert!(!zero.is_trusted_stream_sample());
        let terminal = zero.into_terminal_sample();
        assert!(terminal.is_trusted_stream_sample());
        let mut json = serde_json::to_value(&terminal).unwrap();
        assert!(json.get("terminal_sample").is_none());
        json["terminal_sample"] = serde_json::json!(true);
        let restored: super::Usage = serde_json::from_value(json).unwrap();
        assert!(!restored.terminal_sample);
        for partial in [
            super::Usage {
                cache_read_input_tokens: Some(1),
                ..super::Usage::default()
            },
            super::Usage {
                cache_creation_input_tokens: Some(1),
                ..super::Usage::default()
            },
            super::Usage {
                reasoning_output_tokens: Some(1),
                ..super::Usage::default()
            },
        ] {
            assert!(partial.is_trusted_stream_sample());
        }
    }

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
    fn usage_presence_preserves_explicit_zero_and_legacy_samples() {
        let unknown = Usage::default();
        let reported_zero = Usage {
            reported: true,
            ..Usage::default()
        };
        assert!(!unknown.is_reported());
        assert!(reported_zero.is_reported());
        assert_eq!(
            unknown.display_total_tokens(),
            reported_zero.display_total_tokens()
        );
        assert_eq!(
            serde_json::from_value::<Usage>(serde_json::to_value(&reported_zero).unwrap()).unwrap(),
            reported_zero
        );
        for value in [
            json!({"input_tokens": 1, "output_tokens": 0}),
            json!({"input_tokens": 0, "output_tokens": 0, "total_tokens": 0}),
        ] {
            assert!(
                serde_json::from_value::<Usage>(value)
                    .unwrap()
                    .is_reported()
            );
        }
        assert!(
            !serde_json::from_value::<Usage>(json!({
                "input_tokens": 0, "output_tokens": 0
            }))
            .unwrap()
            .is_reported()
        );
    }

    #[test]
    fn usage_totals_follow_conservative_accounting() {
        let usage = Usage {
            terminal_sample: false,
            unreported_fields: None,
            reported: true,
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
            terminal_sample: false,
            unreported_fields: None,
            reported: true,
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
                terminal_sample: false,
                unreported_fields: None,
                reported: false,
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
            StopReason::PauseTurn,
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
                terminal_sample: false,
                unreported_fields: None,
                reported: true,
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

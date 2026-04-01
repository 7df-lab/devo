use serde::{Deserialize, Serialize};

/// A content block in the model's response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ResponseContent {
    Text(String),
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

/// Token usage statistics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: usize,
    pub output_tokens: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<usize>,
}

/// Why the model stopped generating.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    StopSequence,
}

/// Complete model response (non-streaming).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelResponse {
    pub id: String,
    pub content: Vec<ResponseContent>,
    pub stop_reason: Option<StopReason>,
    pub usage: Usage,
}

/// Incremental events emitted during streaming.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// Start of a new content block.
    ContentBlockStart {
        index: usize,
        content: ResponseContent,
    },
    /// Incremental text delta.
    TextDelta { index: usize, text: String },
    /// Incremental JSON delta for tool input.
    InputJsonDelta { index: usize, partial_json: String },
    /// A content block is complete.
    ContentBlockStop { index: usize },
    /// The full message is complete.
    MessageDone { response: ModelResponse },
    /// Usage update mid-stream.
    UsageDelta(Usage),
}

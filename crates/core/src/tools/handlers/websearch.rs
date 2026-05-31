use async_trait::async_trait;

use crate::contracts::{
    ToolCallError, ToolContext, ToolProgressSender, ToolResult, ToolResultContent,
};
use crate::json_schema::JsonSchema;
use crate::tool_handler::ToolHandler;
use crate::tool_spec::{ToolCapabilityTag, ToolExecutionMode, ToolOutputMode, ToolSpec};

pub struct WebSearchHandler {
    spec: ToolSpec,
}

impl Default for WebSearchHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl WebSearchHandler {
    pub fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "websearch".into(),
                description: "Search the web using an external search provider.".into(),
                input_schema: JsonSchema::object(
                    std::collections::BTreeMap::from([(
                        "query".to_string(),
                        JsonSchema::string(Some("The search query")),
                    )]),
                    Some(vec!["query".to_string()]),
                    None,
                ),
                output_mode: ToolOutputMode::Text,
                execution_mode: ToolExecutionMode::ReadOnly,
                capability_tags: vec![ToolCapabilityTag::NetworkAccess],
                supports_parallel: true,
                preparation_feedback: crate::tool_spec::ToolPreparationFeedback::None,
                display_name: None,
                supports_cancellation: None,
                supports_streaming: None,
            },
        }
    }
}

#[async_trait]
impl ToolHandler for WebSearchHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        _ctx: ToolContext,
        input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let query = input["query"].as_str().unwrap_or("");
        let client = reqwest::Client::new();
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "web_search_exa",
                "arguments": {
                    "query": query,
                    "type": input["type"].as_str().unwrap_or("auto"),
                    "numResults": input["numResults"].as_u64().unwrap_or(8),
                    "livecrawl": input["livecrawl"].as_str().unwrap_or("fallback"),
                    "contextMaxCharacters": input["contextMaxCharacters"].as_u64()
                }
            }
        });

        let res = client
            .post("https://mcp.exa.ai/mcp")
            .json(&payload)
            .send()
            .await
            .map_err(|e| ToolCallError::ExecutionFailed(format!("Search request failed: {e}")))?;

        if !res.status().is_success() {
            let msg = format!("Search error ({})", res.status());
            return Ok(ToolResult::error(
                ToolResultContent::Text(msg.clone()),
                "Search error",
                ToolCallError::ExecutionFailed(msg),
            ));
        }

        let text = res.text().await.map_err(|e| {
            ToolCallError::ExecutionFailed(format!("Failed to read search response: {e}"))
        })?;

        Ok(ToolResult::success(
            ToolResultContent::Text(text),
            "Search completed",
        ))
    }
}

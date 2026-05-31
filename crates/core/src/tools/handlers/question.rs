use async_trait::async_trait;

use crate::contracts::{
    ToolCallError, ToolContext, ToolProgressSender, ToolResult, ToolResultContent,
};
use crate::json_schema::JsonSchema;
use crate::tool_handler::ToolHandler;
use crate::tool_spec::ToolSpec;

pub struct QuestionHandler {
    spec: ToolSpec,
}

impl Default for QuestionHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl QuestionHandler {
    pub fn new() -> Self {
        Self {
            spec: ToolSpec::new(
                "question",
                "Ask the user a question to get clarification or ask for confirmation before proceeding.",
                JsonSchema::object(
                    std::collections::BTreeMap::from([(
                        "question".to_string(),
                        JsonSchema::string(Some("The question to ask the user")),
                    )]),
                    Some(vec!["question".to_string()]),
                    None,
                ),
            ),
        }
    }
}

#[async_trait]
impl ToolHandler for QuestionHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        _ctx: ToolContext,
        input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let question = input["question"].as_str().unwrap_or("");
        Ok(ToolResult::success(
            ToolResultContent::Text(format!("Question for user: {question}")),
            "Question posed",
        ))
    }
}

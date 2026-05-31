use async_trait::async_trait;

use crate::contracts::{
    ToolCallError, ToolContext, ToolProgressSender, ToolResult, ToolResultContent,
};
use crate::json_schema::JsonSchema;
use crate::tool_handler::ToolHandler;
use crate::tool_spec::ToolSpec;

pub struct TaskHandler {
    spec: ToolSpec,
}

impl Default for TaskHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskHandler {
    pub fn new() -> Self {
        Self {
            spec: ToolSpec::new(
                "task",
                "Launch a new agent to handle complex, multi-step tasks autonomously.",
                JsonSchema::object(
                    std::collections::BTreeMap::from([
                        (
                            "description".to_string(),
                            JsonSchema::string(Some("A short (3-5 words) description of the task")),
                        ),
                        (
                            "prompt".to_string(),
                            JsonSchema::string(Some("The task for the agent to perform")),
                        ),
                        (
                            "subagent_type".to_string(),
                            JsonSchema::string(Some("The type of specialized agent to use")),
                        ),
                    ]),
                    Some(vec![
                        "description".to_string(),
                        "prompt".to_string(),
                        "subagent_type".to_string(),
                    ]),
                    None,
                ),
            ),
        }
    }
}

#[async_trait]
impl ToolHandler for TaskHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        _ctx: ToolContext,
        _input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let task_id = uuid::Uuid::new_v4().to_string();
        Ok(ToolResult::success(
            ToolResultContent::Text(format!(
                "Task {task_id} has been launched. Use the task_id to check status or follow up."
            )),
            "Task launched",
        ))
    }
}

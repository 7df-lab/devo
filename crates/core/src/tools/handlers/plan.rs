use async_trait::async_trait;
use serde_json::json;

use crate::contracts::{
    ToolCallError, ToolContext, ToolProgressSender, ToolResult, ToolResultContent,
};
use crate::json_schema::JsonSchema;
use crate::tool_handler::ToolHandler;
use crate::tool_spec::{ToolExecutionMode, ToolOutputMode, ToolSpec};

pub struct PlanHandler {
    spec: ToolSpec,
}

impl Default for PlanHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl PlanHandler {
    pub fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "update_plan".into(),
                description: "Update the current task plan with progress tracking.".into(),
                input_schema: JsonSchema::object(
                    std::collections::BTreeMap::from([
                        (
                            "explanation".to_string(),
                            JsonSchema::string(Some("Brief explanation of the plan changes")),
                        ),
                        (
                            "plan".to_string(),
                            JsonSchema::array(
                                JsonSchema::object(
                                    std::collections::BTreeMap::from([
                                        ("content".to_string(), JsonSchema::string(None)),
                                        ("status".to_string(), JsonSchema::string(None)),
                                        ("priority".to_string(), JsonSchema::string(None)),
                                    ]),
                                    Some(vec![
                                        "content".to_string(),
                                        "status".to_string(),
                                        "priority".to_string(),
                                    ]),
                                    None,
                                ),
                                Some("List of plan items"),
                            ),
                        ),
                    ]),
                    Some(vec!["plan".to_string()]),
                    None,
                ),
                output_mode: ToolOutputMode::Mixed,
                execution_mode: ToolExecutionMode::ReadOnly,
                capability_tags: vec![],
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
impl ToolHandler for PlanHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        _ctx: ToolContext,
        input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let explanation = input
            .get("explanation")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let plan = input
            .get("plan")
            .and_then(|v| v.as_array())
            .ok_or_else(|| ToolCallError::InvalidInput("missing 'plan' field".into()))?;

        let in_progress_count = plan
            .iter()
            .filter(|item| item.get("status").and_then(|v| v.as_str()) == Some("in_progress"))
            .count();
        if in_progress_count > 1 {
            return Ok(ToolResult::error(
                ToolResultContent::Text("At most one step can be in_progress at a time.".into()),
                "Invalid plan",
                ToolCallError::InvalidInput("at most one step can be in_progress".into()),
            ));
        }

        let plan_text = serde_json::to_string_pretty(plan)
            .map_err(|e| ToolCallError::InternalError(e.to_string()))?;
        let content = if explanation.trim().is_empty() {
            plan_text
        } else {
            format!("{explanation}\n\n{plan_text}")
        };

        Ok(ToolResult::success(
            ToolResultContent::Mixed {
                text: Some(content),
                json: Some(json!({
                    "explanation": explanation,
                    "plan": plan,
                })),
            },
            "Plan updated",
        ))
    }
}

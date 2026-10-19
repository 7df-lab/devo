use async_trait::async_trait;
use devo_protocol::CollaborationMode;
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
                description: "Updates the task plan.\nProvide an optional explanation and a list of plan items, each with a step and status.\nAt most one step can be in_progress at a time.".into(),
                input_schema: JsonSchema::object(
                    std::collections::BTreeMap::from([
                        (
                            "explanation".to_string(),
                            JsonSchema::string(Some("Optional explanation for the plan update")),
                        ),
                        (
                            "plan".to_string(),
                            JsonSchema::array(
                                JsonSchema::object(
                                    std::collections::BTreeMap::from([
                                        (
                                            "step".to_string(),
                                            JsonSchema::string(Some(
                                                "Description of the plan step",
                                            )),
                                        ),
                                        (
                                            "status".to_string(),
                                            JsonSchema::string(Some("Status of the step")),
                                        ),
                                    ]),
                                    Some(vec!["step".to_string(), "status".to_string()]),
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
        ctx: ToolContext,
        input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        if ctx.collaboration_mode == CollaborationMode::Plan {
            return Err(ToolCallError::BlockedByMode("plan mode".to_string()));
        }

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
            .filter(|item| {
                matches!(
                    item.get("status").and_then(|v| v.as_str()),
                    Some("in_progress" | "inProgress")
                )
            })
            .count();
        if in_progress_count > 1 {
            return Ok(ToolResult::error(
                ToolResultContent::Text("At most one step can be in_progress at a time.".into()),
                "Invalid plan",
                ToolCallError::InvalidInput("at most one step can be in_progress".into()),
            ));
        }

        // Canonicalize items: accept the step text under `step` / `content` /
        // `step_name` (providers routinely emit `step_name` despite the
        // schema) and re-emit as `step` so downstream plan parsing never
        // depends on the alias. Items without any step text are rejected with
        // a clear message instead of silently producing an empty plan.
        let mut canonical_plan = Vec::with_capacity(plan.len());
        for (index, item) in plan.iter().enumerate() {
            let step = ["step", "content", "step_name"]
                .iter()
                .find_map(|key| item.get(*key))
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|step| !step.is_empty());
            let Some(step) = step else {
                return Ok(ToolResult::error(
                    ToolResultContent::Text(format!(
                        "Plan item {index} is missing a step description: each item needs a \
                         non-empty 'step' string (aliases 'content' and 'step_name' are \
                         accepted) plus a 'status'."
                    )),
                    "Invalid plan",
                    ToolCallError::InvalidInput(format!(
                        "plan item {index} is missing a non-empty 'step' string"
                    )),
                ));
            };
            canonical_plan.push(json!({
                "step": step,
                "status": item
                    .get("status")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("pending"),
            }));
        }

        let plan_text = serde_json::to_string_pretty(&canonical_plan)
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
                    "plan": canonical_plan,
                })),
            },
            "Plan updated",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn run_handler(plan: serde_json::Value) -> Result<ToolResult, ToolCallError> {
        let handler = PlanHandler::new();
        handler
            .handle(
                crate::contracts::ToolContext {
                    output_store: None,
                    tool_call_id: crate::invocation::ToolCallId("call".to_string()),
                    session_id: "session-1".into(),
                    turn_id: Some("turn-1".into()),
                    workspace_root: std::path::PathBuf::from("."),
                    budgets: crate::contracts::ToolBudgets {
                        output_limit_bytes: 1024,
                        wall_time_limit_ms: None,
                    },
                    cancel_token: tokio_util::sync::CancellationToken::new(),
                    agent_scope: crate::contracts::ToolAgentScope::Parent,
                    collaboration_mode: devo_protocol::CollaborationMode::Build,
                    agent_coordinator: None,
                    client_filesystem: None,
                    file_read_ledger: None,
                    network_proxy: None,
                    network_no_proxy: None,
                    sandbox_permission_overlay: None,
                    sandbox_profile: None,
                    kernel: None,
                    python_cell_first_wait_ms: None,
                    python_cell_watch: None,
                    python_cell_completion: None,
                    session_dir: None,
                },
                serde_json::json!({ "plan": plan }),
                None,
            )
            .await
    }

    #[tokio::test]
    async fn canonicalizes_step_name_alias_to_step() {
        // Providers emit `step_name` despite the schema; the result must echo
        // a canonical `step` key so downstream plan parsing never depends on
        // the alias.
        let result = run_handler(
            serde_json::json!([{ "step_name": "Debug-R8-PARSE", "status": "pending" }]),
        )
        .await
        .expect("handler succeeds");
        match result.content {
            ToolResultContent::Mixed {
                json: Some(json), ..
            } => {
                assert_eq!(
                    json.get("plan"),
                    Some(&serde_json::json!([{ "step": "Debug-R8-PARSE", "status": "pending" }])),
                    "plan must be canonicalized to `step`"
                );
            }
            other => panic!("expected mixed content with json, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn rejects_items_without_any_step_text() {
        let result = run_handler(serde_json::json!([{ "status": "pending" }]))
            .await
            .expect("handler returns a tool result");
        assert!(
            matches!(
                &result.structured_status,
                crate::contracts::ToolTerminalStatus::Failed(ToolCallError::InvalidInput(_))
            ),
            "missing step text must be an invalid-input failure, got {:?}",
            result.structured_status
        );
    }
}

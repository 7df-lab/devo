use async_trait::async_trait;
use devo_protocol::AgentContextMode;
use devo_protocol::CollaborationMode;
use devo_protocol::RequestUserInputArgs;
use devo_protocol::RequestUserInputQuestion;

use crate::contracts::{
    ToolCallError, ToolContext, ToolProgressSender, ToolResult, ToolResultContent,
};
use crate::json_schema::JsonSchema;
use crate::tool_handler::ToolHandler;
use crate::tool_spec::ToolOutputMode;
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
        let mut spec = ToolSpec::new(
            "request_user_input",
            "Use this tool when you need to ask the user questions during execution. This allows you to gather user preferences or requirements, clarify ambiguous instructions, get decisions on implementation choices as you work, or offer choices to the user about what direction to take.\n\nUsage notes:\n- Users will always be able to select Other to provide custom text input when the UI supports it.\n- If you recommend a specific option, make that the first option in the list and add \"(Recommended)\" at the end of the label.\n- In Plan Mode, use this tool to clarify requirements or choose between approaches BEFORE finalizing your plan.\n- Do NOT use this tool to ask \"Is my plan ready?\" or \"Should I proceed?\".\n- IMPORTANT: Do not reference \"the plan\" in your questions because the user cannot see the plan in the UI until plan approval is requested through the appropriate Plan Mode flow.",
            JsonSchema::object(
                std::collections::BTreeMap::from([("questions".to_string(), questions_schema())]),
                Some(vec!["questions".to_string()]),
                Some(false),
            ),
        );
        spec.output_mode = ToolOutputMode::StructuredJson;
        Self { spec }
    }
}

#[async_trait]
impl ToolHandler for QuestionHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        if ctx.collaboration_mode != CollaborationMode::Plan
            && ctx.agent_context_mode != AgentContextMode::DeepResearch
        {
            return Err(ToolCallError::BlockedByMode(
                "plan mode or deep research".to_string(),
            ));
        }

        let args = request_user_input_args(input)?;
        let turn_id = ctx.turn_id.clone().ok_or_else(|| {
            ToolCallError::ExecutionFailed("request_user_input requires an active turn".to_string())
        })?;
        let coordinator = ctx.agent_coordinator.clone().ok_or_else(|| {
            ToolCallError::ExecutionFailed(
                "request_user_input is unavailable in this runtime".to_string(),
            )
        })?;
        let response = coordinator
            .request_user_input(
                ctx.session_id.clone(),
                turn_id,
                ctx.tool_call_id.0.clone(),
                args,
            )
            .await?;
        Ok(ToolResult::success(
            ToolResultContent::Json(
                serde_json::to_value(response)
                    .map_err(|error| ToolCallError::InternalError(error.to_string()))?,
            ),
            "User input received",
        ))
    }
}

fn questions_schema() -> JsonSchema {
    let option_schema = JsonSchema::object(
        std::collections::BTreeMap::from([
            (
                "label".to_string(),
                JsonSchema::string(Some("Short option label shown to the user")),
            ),
            (
                "description".to_string(),
                JsonSchema::string(Some("One sentence describing the option tradeoff")),
            ),
        ]),
        Some(vec!["label".to_string(), "description".to_string()]),
        Some(false),
    );
    let question_schema = JsonSchema::object(
        std::collections::BTreeMap::from([
            (
                "id".to_string(),
                JsonSchema::string(Some("Stable snake_case identifier for this question")),
            ),
            (
                "header".to_string(),
                JsonSchema::string(Some("Short header label, 12 or fewer characters")),
            ),
            (
                "question".to_string(),
                JsonSchema::string(Some("Single sentence prompt shown to the user")),
            ),
            (
                "isOther".to_string(),
                JsonSchema::boolean(Some("Whether a free-form Other answer is allowed")),
            ),
            (
                "isSecret".to_string(),
                JsonSchema::boolean(Some("Whether free-form text should be treated as secret")),
            ),
            (
                "options".to_string(),
                JsonSchema::array(option_schema, Some("Mutually exclusive answer options")),
            ),
        ]),
        Some(vec![
            "id".to_string(),
            "header".to_string(),
            "question".to_string(),
        ]),
        Some(false),
    );
    JsonSchema::array(question_schema, Some("Questions to show the user"))
}

fn request_user_input_args(
    input: serde_json::Value,
) -> Result<RequestUserInputArgs, ToolCallError> {
    if input.get("questions").is_some() {
        serde_json::from_value(input)
            .map_err(|error| ToolCallError::InvalidInput(error.to_string()))
    } else if let Some(question) = input.get("question").and_then(serde_json::Value::as_str) {
        Ok(RequestUserInputArgs {
            questions: vec![RequestUserInputQuestion {
                id: "question".to_string(),
                header: "Question".to_string(),
                question: question.to_string(),
                is_other: true,
                is_secret: false,
                options: None,
            }],
        })
    } else {
        Err(ToolCallError::InvalidInput(
            "missing 'questions' field".to_string(),
        ))
    }
}

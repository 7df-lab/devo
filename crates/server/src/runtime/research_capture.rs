use std::collections::HashMap;

use devo_core::ItemId;

use super::research_context::ResearchClarificationContext;

#[derive(Default)]
pub(super) struct ResearchQueryCapture {
    pub(super) text: String,
    pub(super) assistant: StreamedTextItem,
    pub(super) pending_tools: HashMap<String, PendingResearchToolCall>,
    pub(super) final_report_write: Option<FinalReportWrite>,
    pub(super) reasoning: StreamedTextItem,
    pub(super) usage_invocation_index: usize,
    pub(super) turn_completed: bool,
}

#[derive(Default)]
pub(super) struct ClarificationQueryCapture {
    pub(super) text: String,
    pub(super) artifact: StreamedTextItem,
    pub(super) pending_request_user_input_questions: HashMap<String, Vec<(String, String)>>,
    pub(super) request_user_input_exchanges: Vec<ResearchClarificationContext>,
    pub(super) clarifications: Vec<ResearchClarificationContext>,
    pub(super) reasoning: StreamedTextItem,
    pub(super) usage_invocation_index: usize,
}

#[derive(Default)]
pub(super) struct SupervisorQueryCapture {
    pub(super) text: String,
    pub(super) artifact: StreamedTextItem,
    pub(super) pending_tools: HashMap<String, PendingResearchToolCall>,
    pub(super) reasoning: StreamedTextItem,
    pub(super) usage_invocation_index: usize,
    pub(super) spawned_worker_count: usize,
}

#[derive(Default)]
pub(super) struct ResearchArtifactQueryCapture {
    pub(super) text: String,
    pub(super) artifact: StreamedTextItem,
    pub(super) reasoning: StreamedTextItem,
    pub(super) usage_invocation_index: usize,
    pub(super) turn_completed: bool,
}

pub(super) enum ResearchStageCapture<'a> {
    Clarification(&'a mut ClarificationQueryCapture),
    Artifact(&'a mut ResearchArtifactQueryCapture),
    Supervisor(&'a mut SupervisorQueryCapture),
    FinalReport(&'a mut ResearchQueryCapture),
}

pub(super) struct PendingResearchToolCall {
    pub(super) item_id: ItemId,
    pub(super) item_seq: u64,
    pub(super) tool_name: String,
    pub(super) input: serde_json::Value,
}

#[derive(Debug, Clone)]
pub(super) struct FinalReportWrite {
    pub(super) path: String,
    pub(super) content: String,
}

#[derive(Debug, Default)]
pub(super) struct StreamedTextItem {
    pub(super) item_id: Option<ItemId>,
    pub(super) item_seq: Option<u64>,
    pub(super) text: String,
}

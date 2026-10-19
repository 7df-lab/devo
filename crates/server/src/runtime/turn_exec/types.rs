use devo_protocol::native::ids::{ItemId as NativeItemId, TurnId as NativeTurnId};
use devo_protocol::native::usage::TurnUsage;

/// Inputs captured at turn-start time and handed to the background turn executor.
pub(crate) struct ExecuteTurnRequest {
    /// Runtime session that owns the turn and receives emitted items, usage, and status updates.
    pub(crate) session_id: devo_core::SessionId,
    /// Native-first turn state checked out with the execution working set.
    pub(crate) turn: crate::turn::RuntimeTurn,
    /// Resolved model, provider, reasoning, tool, web, and token-budget settings for this turn.
    pub(crate) turn_config: devo_core::TurnConfig,
    /// User-facing rendering of the submitted input. Visible turns persist this as the displayed
    /// user message; hidden continuation turns keep it out of the transcript.
    pub(crate) display_input: String,
    /// Stable domain ID for one logical user message, when supplied by the client.
    pub(crate) client_user_message_id: Option<String>,
    /// Native resolved prompt text. Visible turns push this as the user-role message when the
    /// input resolver did not return structured `input_messages`.
    pub(crate) input: String,
    /// Structured user-role messages produced by input resolution, such as expanded skill content.
    /// When non-empty, these are pushed instead of the single `input` string.
    pub(crate) input_messages: Vec<String>,
    /// Base64-encoded image parts resolved from local image inputs.
    pub(crate) input_images: Vec<devo_protocol::PromptImagePart>,
    /// Filesystem paths for successfully resolved local images (persisted on
    /// the user-message [`TextItem`] for history rebuild / projection).
    pub(crate) input_image_paths: Vec<std::path::PathBuf>,
    /// Collaboration mode to install on the core session for this query; it also drives
    /// mode-specific stream handling such as proposed-plan parsing.
    pub(crate) collaboration_mode: devo_protocol::CollaborationMode,
    /// Controls whether this executor emits/pushes a visible user message or runs hidden work such
    /// as goal continuation, and carries the hidden goal context when needed.
    pub(crate) input_mode: super::super::TurnInputMode,
    /// When true, `prepare_turn_execution_for_actor` skips the transcript UserMessage emit
    /// because the turn/start handler already projected it (before `TurnStarted`).
    pub(crate) user_message_already_emitted: bool,
}

pub(super) struct PendingToolCall {
    pub(super) item_id: Option<NativeItemId>,
    pub(super) item_seq: Option<u64>,
    pub(super) input: serde_json::Value,
    pub(super) display_kind: ToolDisplayKind,
    pub(super) command: String,
    /// When the tool call started; reported when the turn aborts mid-tool so
    /// the model knows how long the interrupted call had been running.
    pub(super) started_at: std::time::Instant,
}

pub(crate) struct TurnEventStreamSummary {
    /// Aggregate usage for the complete turn, including tool-use model legs.
    pub(crate) turn_usage: Option<TurnUsage>,
    /// Usage from the most recent individual model invocation.
    pub(crate) latest_query_usage: Option<TurnUsage>,
    pub(crate) stop_reason: Option<devo_core::StopReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToolDisplayKind {
    CommandExecution,
    Generic,
}

impl ToolDisplayKind {
    pub(super) fn for_tool_name(name: &str) -> Self {
        if super::tool_display::is_unified_exec_tool(name) {
            Self::CommandExecution
        } else {
            Self::Generic
        }
    }

    pub(super) fn is_command_execution(self) -> bool {
        self == Self::CommandExecution
    }
}

pub(super) struct ToolStartItem {
    pub(super) native_item: devo_protocol::native::item::Item,
}

pub(crate) struct TurnQueryOutcome {
    pub(crate) result: Result<(), devo_core::AgentError>,
    pub(crate) session_total_input_tokens: usize,
    pub(crate) session_total_output_tokens: usize,
    pub(crate) session_total_tokens: usize,
    pub(crate) session_total_cache_creation_tokens: usize,
    pub(crate) session_total_cache_read_tokens: usize,
    pub(crate) session_last_input_tokens: usize,
    pub(crate) session_prompt_token_estimate: usize,
}

pub(super) struct QueuedTurnInput {
    /// Native queue entry id of the drained entry (`queue/updated`
    /// `drained` notification, 01 §4.3).
    pub(super) queued_item_id: devo_protocol::native::ids::QueueItemId,
    pub(super) client_user_message_id: Option<String>,
    pub(super) display_input: String,
    pub(super) input_text: String,
    pub(super) input_messages: Vec<String>,
    pub(super) input_images: Vec<devo_protocol::PromptImagePart>,
    pub(super) input_image_paths: Vec<std::path::PathBuf>,
    pub(super) collaboration_mode: devo_protocol::CollaborationMode,
    pub(super) model_selection: Option<String>,
    pub(super) subagent_usage_owner: Option<(devo_core::SessionId, Option<NativeTurnId>)>,
}

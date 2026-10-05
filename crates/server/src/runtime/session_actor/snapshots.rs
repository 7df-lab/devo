use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use devo_core::SessionConfig;
use devo_protocol::PendingInputItem;
use devo_protocol::native::ids::{SessionId, TurnId};
use devo_protocol::native::turn::TurnKind;

use crate::runtime_session_summary::RuntimeSessionSummary;
use crate::session_context::SessionRuntimeContext;
use crate::turn::RuntimeTurn;

/// Snapshot used when reserving or queueing a turn on a session actor.
#[derive(Clone)]
pub(crate) struct TurnReservationSnapshot {
    pub(crate) max_turns: Option<u32>,
    pub(crate) active_turn: Option<RuntimeTurn>,
    pub(crate) latest_turn: Option<RuntimeTurn>,
    pub(crate) ephemeral: bool,
    pub(crate) parent_session_id: Option<SessionId>,
    pub(crate) summary: RuntimeSessionSummary,
    pub(crate) runtime_context: Arc<SessionRuntimeContext>,
    pub(crate) pending_turn_queue: Arc<StdMutex<VecDeque<PendingInputItem>>>,
    pub(crate) steer_input_queue: Arc<StdMutex<VecDeque<PendingInputItem>>>,
}

/// Hook runner inputs derived from session actor state.
#[derive(Clone)]
pub(crate) struct HookContextSnapshot {
    pub(crate) runtime_context: Arc<SessionRuntimeContext>,
    pub(crate) rollout_path: Option<PathBuf>,
    pub(crate) summary: RuntimeSessionSummary,
    pub(crate) config: SessionConfig,
}

/// Fields needed to persist a turn line to rollout storage.
#[derive(Clone)]
pub(crate) struct TurnPersistenceSnapshot {
    pub(crate) rollout_path: Option<PathBuf>,
}

/// Sandbox context for session-owned command execution.
#[derive(Clone)]
pub(crate) struct ShellExecContextSnapshot {
    pub(crate) sandbox_profile: Option<String>,
}

/// Context for async title generation.
#[derive(Clone)]
pub(crate) struct TitleGenerationContext {
    pub(crate) model_selection: Option<String>,
    pub(crate) reasoning_effort_selection: Option<String>,
    pub(crate) title_state: devo_core::SessionTitleState,
    pub(crate) runtime_context: Arc<SessionRuntimeContext>,
}

/// Pending turn queue broadcast snapshot.
#[derive(Clone, Default)]
pub(crate) struct PendingQueueSnapshot {
    pub(crate) pending_count: usize,
}

/// Fields returned by session/resume without locking the actor mailbox.
#[derive(Clone)]
pub(crate) struct SessionResumeSnapshot {
    pub(crate) summary: RuntimeSessionSummary,
    pub(crate) latest_turn: Option<RuntimeTurn>,
    pub(crate) loaded_item_count: u64,
    pub(crate) history_items: Vec<crate::session::SessionHistoryEntry>,
    pub(crate) pending_texts: Vec<String>,
}

/// Popped queued turn input for follow-up execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct QueuedTurnInputData {
    pub(crate) queued_input_id: devo_core::QueueItemId,
    pub(crate) client_user_message_id: Option<String>,
    pub(crate) display_input: String,
    pub(crate) input_text: String,
    pub(crate) input_messages: Vec<String>,
    pub(crate) input_images: Vec<devo_protocol::PromptImagePart>,
    pub(crate) input_image_paths: Vec<std::path::PathBuf>,
    pub(crate) collaboration_mode: devo_protocol::CollaborationMode,
    pub(crate) model_selection: Option<String>,
    pub(crate) subagent_usage_owner: Option<(SessionId, Option<TurnId>)>,
}

/// Turn kind and durable path before persisting an item.
#[derive(Clone)]
pub(crate) struct PersistItemPrep {
    pub(crate) turn_kind: TurnKind,
    pub(crate) rollout_path: Option<PathBuf>,
    pub(crate) transcript_leaf_id: Option<devo_protocol::native::ids::ItemId>,
    pub(crate) leaf_epoch: u64,
}

/// Deferred streaming items captured during graceful shutdown.
#[derive(Clone, Default)]
pub(crate) struct ShutdownDeferredSnapshot {
    pub(crate) deferred_assistant: Option<(devo_protocol::native::ids::ItemId, u64, String)>,
    pub(crate) deferred_reasoning: Option<(devo_protocol::native::ids::ItemId, u64, String)>,
    pub(crate) active_turn: Option<RuntimeTurn>,
    pub(crate) active_turn_id: Option<TurnId>,
    pub(crate) rollout_path: Option<PathBuf>,
}

use std::{collections::VecDeque, path::PathBuf, sync::Arc};

use tokio::{sync::Mutex, task::JoinHandle};

use clawcr_core::{
    Model, ModelCatalog, SessionConfig, SessionId, SessionRecord, SessionState, TurnConfig,
    default_base_instructions,
};
use clawcr_provider::ModelProviderSDK;
use clawcr_tools::ToolRegistry;

use crate::{
    session::{SessionHistoryItem, SessionSummary},
    turn::{SteerInputRecord, TurnSummary},
};

/// Shared server-owned runtime dependencies used by live turn execution.
pub struct ServerRuntimeDependencies {
    /// Provider used for all model requests.
    pub(crate) provider: Arc<dyn ModelProviderSDK>,
    /// Shared built-in tool registry used by turn execution.
    pub(crate) registry: Arc<ToolRegistry>,
    /// Default model applied when no model override is present.
    pub(crate) default_model: String,
    /// Model catalog used to resolve builtin prompt metadata.
    pub(crate) model_catalog: Arc<dyn ModelCatalog>,
}

impl ServerRuntimeDependencies {
    /// Creates a new bundle of runtime dependencies for the transport server.
    pub fn new(
        provider: Arc<dyn ModelProviderSDK>,
        registry: Arc<ToolRegistry>,
        default_model: String,
        model_catalog: Arc<dyn ModelCatalog>,
    ) -> Self {
        Self {
            provider,
            registry,
            default_model,
            model_catalog,
        }
    }

    /// Creates an initial core session state for a newly created server session.
    pub(crate) fn new_session_state(&self, session_id: SessionId, cwd: PathBuf) -> SessionState {
        let mut state = SessionState::new(SessionConfig::default(), cwd);
        state.id = session_id.to_string();
        state
    }

    /// Resolves one runtime model for a turn, applying the server default when needed.
    pub(crate) fn resolve_turn_model(&self, requested_model: Option<&str>) -> Model {
        if let Some(model) = requested_model.and_then(|requested| self.model_catalog.get(requested))
        {
            return model.clone();
        }

        self.model_catalog
            .resolve_for_turn(Some(&self.default_model))
            .or_else(|_| self.model_catalog.resolve_for_turn(None))
            .cloned()
            .unwrap_or_else(|_| Model {
                slug: self.default_model.clone(),
                base_instructions: default_base_instructions().to_string(),
                ..Model::default()
            })
    }

    /// Resolves the full turn configuration used by the core query loop.
    pub(crate) fn resolve_turn_config(
        &self,
        requested_model: Option<&str>,
        thinking_selection: Option<String>,
    ) -> TurnConfig {
        TurnConfig {
            model: self.resolve_turn_model(requested_model),
            thinking_selection,
        }
    }
}

/// Mutable per-session runtime state owned by the server.
pub(crate) struct RuntimeSession {
    /// Canonical persisted session metadata when the session is durable.
    pub(crate) record: Option<SessionRecord>,
    /// Transport-facing summary exposed over the API.
    pub(crate) summary: SessionSummary,
    /// Canonical core session state used by the query loop.
    pub(crate) core_session: Arc<Mutex<SessionState>>,
    /// Currently active turn, if any.
    pub(crate) active_turn: Option<TurnSummary>,
    /// Latest terminal turn summary for the session.
    pub(crate) latest_turn: Option<TurnSummary>,
    /// Number of items loaded or appended for the session.
    pub(crate) loaded_item_count: u64,
    /// Replay-friendly ordered history used by interactive clients during session resume.
    pub(crate) history_items: Vec<SessionHistoryItem>,
    /// Pending same-turn steering inputs.
    pub(crate) steering_queue: VecDeque<SteerInputRecord>,
    /// Live query task for the active turn.
    pub(crate) active_task: Option<JoinHandle<()>>,
    /// Monotonic session-scoped item sequence counter.
    pub(crate) next_item_seq: u64,
}

impl RuntimeSession {
    /// Wraps a new runtime session in an async mutex for storage in the session map.
    pub(crate) fn shared(self) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(self))
    }
}

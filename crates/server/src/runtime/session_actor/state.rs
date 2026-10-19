use std::collections::HashMap;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use devo_core::SessionConfig;
use devo_core::SessionState;
use devo_core::tools::ToolRegistry;
use devo_protocol::native::ids::{SessionId, TurnId};

use crate::execution::PersistedTurnItem;
use crate::replay_hydrate::ReplayedTurn;
use crate::runtime::RuntimeSession;
use crate::runtime_session_summary::RuntimeSessionSummary;
use crate::session::SessionHistoryEntry;
use crate::session_context::SessionRuntimeContext;
use crate::turn::RuntimeTurn;

use super::turn_inline::TurnInlineState;

/// Immutable parent snapshot used by `spawn_agent` during an active parent turn.
#[derive(Clone)]
pub(crate) struct SpawnSnapshot {
    pub(crate) parent_summary: RuntimeSessionSummary,
    pub(crate) parent_config: SessionConfig,
    pub(crate) stable_items: Vec<PersistedTurnItem>,
    pub(crate) parent_active_turn: Option<RuntimeTurn>,
    pub(crate) parent_latest_turn: Option<RuntimeTurn>,
    pub(crate) parent_active_turn_id: Option<devo_protocol::native::ids::TurnId>,
    pub(crate) parent_tool_registry: Option<Arc<ToolRegistry>>,
    pub(crate) runtime_context: Arc<SessionRuntimeContext>,
    pub(crate) pending_turn_queue: Arc<StdMutex<VecDeque<devo_protocol::PendingInputItem>>>,
    pub(crate) steer_input_queue: Arc<StdMutex<VecDeque<devo_protocol::PendingInputItem>>>,
}

/// Approval caches cloned at turn start for permission checks while the turn
/// task owns the working copy.
#[derive(Clone, Default)]
pub(crate) struct ApprovalCacheSnapshot {
    pub(crate) session_approval_cache: crate::execution::ApprovalGrantCache,
    pub(crate) turn_approval_cache: crate::execution::ApprovalGrantCache,
}

#[derive(Clone, Default)]
pub(crate) struct DeferredItems {
    pub(crate) assistant: Option<(devo_protocol::native::ids::ItemId, u64, String)>,
    pub(crate) reasoning: Option<(devo_protocol::native::ids::ItemId, u64, String)>,
}

use tokio::sync::Mutex as TokioMutex;

/// Streaming-era mutable fields touched by the turn event bridge.
#[derive(Default)]
pub(crate) struct SessionStreamState {
    pub(crate) deferred_assistant: Option<(devo_protocol::native::ids::ItemId, u64, String)>,
    pub(crate) deferred_reasoning: Option<(devo_protocol::native::ids::ItemId, u64, String)>,
    pub(crate) turn_inline: Option<TurnInlineState>,
}

impl SessionStreamState {
    pub(crate) fn take_deferred_items(&mut self) -> DeferredItems {
        DeferredItems {
            assistant: self.deferred_assistant.take(),
            reasoning: self.deferred_reasoning.take(),
        }
    }
}

/// Per-session state owned exclusively by a `SessionActor` task.
///
/// Durable location is [`Self::rollout_path`]; live metadata is Native
/// [`RuntimeSessionSummary`]. Legacy `SessionRecord` is not actor-owned.
pub(crate) struct SessionActorState {
    pub(crate) runtime_context: Arc<SessionRuntimeContext>,
    /// Absolute rollout JSONL path for durable sessions (`None` when ephemeral).
    pub(crate) rollout_path: Option<PathBuf>,
    /// Cross-process exclusive lock on the rollout, held while this actor
    /// owns the session. Dropping the actor releases it (eviction/exit);
    /// the OS releases it if the process dies.
    pub(crate) session_file_lock: Option<crate::runtime::session_cache::SessionFileLock>,
    pub(crate) summary: RuntimeSessionSummary,
    pub(crate) config: SessionConfig,
    pub(crate) core: SessionState,
    pub(crate) stream: Arc<TokioMutex<SessionStreamState>>,
    pub(crate) active_turn: Option<RuntimeTurn>,
    pub(crate) latest_turn: Option<RuntimeTurn>,
    pub(crate) loaded_item_count: u64,
    pub(crate) history_items: Vec<SessionHistoryEntry>,
    pub(crate) persisted_turn_items: Vec<PersistedTurnItem>,
    pub(crate) latest_compaction_snapshot: Option<devo_core::CompactionSnapshotLine>,
    /// See [`RuntimeSession::turns_by_id`] — Native turn snapshots for fork/cuts.
    pub(crate) turns_by_id: HashMap<TurnId, ReplayedTurn>,
    pub(crate) pending_turn_queue: Arc<StdMutex<VecDeque<devo_protocol::PendingInputItem>>>,
    pub(crate) steer_input_queue: Arc<StdMutex<VecDeque<devo_protocol::PendingInputItem>>>,
    pub(crate) agent_tool_policy: devo_protocol::AgentToolPolicy,
    pub(crate) max_turns: Option<u32>,
    pub(crate) next_item_seq: u64,
    pub(crate) first_user_input: Option<String>,
    pub(crate) tool_registry: Option<Arc<ToolRegistry>>,
    /// Session-scoped ledger of files read/written by tools (used by `edit`).
    pub(crate) file_read_ledger: Arc<devo_core::tools::FileReadLedger>,
    /// Session-scoped RLM kernel (created on the turn task; shared via Arc).
    pub(crate) kernel: Option<Arc<devo_kernel::KernelSession>>,
    pub(crate) session_approval_cache: crate::execution::ApprovalGrantCache,
    pub(crate) turn_approval_cache: crate::execution::ApprovalGrantCache,
    pub(crate) session_context_recorded: bool,
    /// Current transcript-tree tip (leafId parity with the tree wire).
    pub(crate) transcript_leaf_id: Option<devo_protocol::native::ids::ItemId>,
    /// Last SessionLeaf epoch written for this session.
    pub(crate) leaf_epoch: u64,
}

impl SessionActorState {
    pub(crate) fn session_id(&self) -> SessionId {
        self.summary.session_id()
    }

    pub(crate) fn parent_session_id(&self) -> Option<SessionId> {
        self.summary.parent_session_id()
    }

    pub(crate) fn sync_native_runtime_fields(&mut self) {
        use devo_protocol::native::session::SessionStatus;

        self.summary.active_turn_id = self.active_turn.as_ref().map(|turn| turn.native.id);
        self.summary.status = if self.summary.active_turn_id.is_some() {
            SessionStatus::Active
        } else {
            SessionStatus::Idle
        };
        self.summary.queued_count = self
            .pending_turn_queue
            .lock()
            .expect("pending turn queue mutex should not be poisoned")
            .len()
            .try_into()
            .unwrap_or(u32::MAX);
        self.summary.settings.sandbox_profile = self.config.sandbox_profile.clone();
        self.summary.sync_activity();
    }

    pub(crate) fn approval_cache_snapshot(&self) -> ApprovalCacheSnapshot {
        ApprovalCacheSnapshot {
            session_approval_cache: self.session_approval_cache.clone(),
            turn_approval_cache: self.turn_approval_cache.clone(),
        }
    }

    pub(crate) fn spawn_snapshot(&self) -> SpawnSnapshot {
        let fork_turns_all = true;
        let stable_items = if fork_turns_all {
            let active_turn_id = self.active_turn.as_ref().map(|turn| *turn.native_turn_id());
            self.persisted_turn_items
                .iter()
                .filter(|item| {
                    active_turn_id
                        .as_ref()
                        .is_none_or(|turn_id| item.turn_id != *turn_id)
                })
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        SpawnSnapshot {
            parent_summary: self.summary.clone(),
            parent_config: self.config.clone(),
            stable_items,
            parent_active_turn: self.active_turn.clone(),
            parent_latest_turn: self.latest_turn.clone(),
            parent_active_turn_id: self
                .active_turn
                .as_ref()
                .map(RuntimeTurn::turn_id)
                .or_else(|| self.latest_turn.as_ref().map(RuntimeTurn::turn_id)),
            parent_tool_registry: self.tool_registry.clone(),
            runtime_context: Arc::clone(&self.runtime_context),
            pending_turn_queue: Arc::clone(&self.pending_turn_queue),
            steer_input_queue: Arc::clone(&self.steer_input_queue),
        }
    }

    pub(crate) fn from_runtime_session(session: RuntimeSession) -> Self {
        let core = Arc::try_unwrap(session.core_session)
            .unwrap_or_else(|_| {
                panic!("session core_session should have a single owner when starting actor")
            })
            .into_inner();
        let mut state = Self {
            runtime_context: session.runtime_context,
            rollout_path: session.rollout_path,
            // Cross-process lock is taken by the loader (session_cache), not
            // by struct conversion; `None` until a load path sets it.
            session_file_lock: None,
            summary: session.summary,
            config: session.config,
            core,
            stream: Arc::new(TokioMutex::new(SessionStreamState {
                deferred_assistant: session.deferred_assistant,
                deferred_reasoning: session.deferred_reasoning,
                turn_inline: None,
            })),
            active_turn: session.active_turn,
            latest_turn: session.latest_turn,
            loaded_item_count: session.loaded_item_count,
            history_items: session.history_items,
            persisted_turn_items: session.persisted_turn_items,
            latest_compaction_snapshot: session.latest_compaction_snapshot,
            turns_by_id: session.turns_by_id,
            pending_turn_queue: session.pending_turn_queue,
            steer_input_queue: session.steer_input_queue,
            agent_tool_policy: session.agent_tool_policy,
            max_turns: session.max_turns,
            next_item_seq: session.next_item_seq,
            first_user_input: session.first_user_input,
            tool_registry: session.tool_registry,
            file_read_ledger: session.file_read_ledger,
            kernel: None,
            session_approval_cache: session.session_approval_cache,
            turn_approval_cache: session.turn_approval_cache,
            session_context_recorded: session.session_context_recorded,
            transcript_leaf_id: None,
            leaf_epoch: 0,
        };
        if let Some(path) = state.rollout_path.as_ref()
            && let Ok((leaf_id, leaf_epoch)) = devo_core::read_rollout_session_leaf(path)
        {
            state.transcript_leaf_id = leaf_id;
            state.leaf_epoch = leaf_epoch;
        }
        state.sync_native_runtime_fields();
        state
    }

    pub(crate) fn to_runtime_session_from_stream(
        &self,
        stream: &SessionStreamState,
    ) -> RuntimeSession {
        RuntimeSession {
            runtime_context: Arc::clone(&self.runtime_context),
            rollout_path: self.rollout_path.clone(),
            summary: self.summary.clone(),
            config: self.config.clone(),
            core_session: Arc::new(tokio::sync::Mutex::new(self.core.snapshot_for_export())),
            active_turn: self.active_turn.clone(),
            latest_turn: self.latest_turn.clone(),
            loaded_item_count: self.loaded_item_count,
            history_items: self.history_items.clone(),
            persisted_turn_items: self.persisted_turn_items.clone(),
            latest_compaction_snapshot: self.latest_compaction_snapshot.clone(),
            turns_by_id: self.turns_by_id.clone(),
            pending_turn_queue: Arc::clone(&self.pending_turn_queue),
            steer_input_queue: Arc::clone(&self.steer_input_queue),
            agent_tool_policy: self.agent_tool_policy,
            max_turns: self.max_turns,
            deferred_assistant: stream.deferred_assistant.clone(),
            deferred_reasoning: stream.deferred_reasoning.clone(),
            next_item_seq: self.next_item_seq,
            first_user_input: self.first_user_input.clone(),
            tool_registry: self.tool_registry.clone(),
            file_read_ledger: Arc::clone(&self.file_read_ledger),
            session_approval_cache: self.session_approval_cache.clone(),
            turn_approval_cache: self.turn_approval_cache.clone(),
            session_context_recorded: self.session_context_recorded,
        }
    }
}

impl From<RuntimeSession> for SessionActorState {
    fn from(session: RuntimeSession) -> Self {
        Self::from_runtime_session(session)
    }
}

use std::path::PathBuf;
use std::sync::Arc;

use devo_core::CompactionSnapshotLine;
use devo_protocol::CollaborationMode;
use devo_protocol::native::ids::{ItemId as NativeItemId, TurnId as NativeTurnId};
use devo_protocol::native::turn::TurnKind;
use devo_protocol::native::usage::TurnUsage;

use crate::execution::ApprovalGrantCache;
use crate::execution::PersistedTurnItem;
use crate::runtime_session_summary::RuntimeSessionSummary;
use crate::session::SessionHistoryEntry;
use crate::turn::RuntimeTurn;

use super::SessionActorState;
use super::snapshots::HookContextSnapshot;

/// Mutable session fields updated during an active turn without mailbox round-trips.
///
/// Transient scratch state registered in `ActiveTurnRegistry` while the turn
/// task owns a [`super::TurnWorkingSet`]. Merges into durable actor state when
/// the turn completes via `MergeTurn`.
pub(crate) struct TurnInlineState {
    pub(crate) turn_id: NativeTurnId,
    pub(crate) turn_kind: TurnKind,
    pub(crate) next_item_seq: u64,
    pub(crate) loaded_item_count: u64,
    pub(crate) persisted_turn_items: Vec<PersistedTurnItem>,
    pub(crate) history_items: Vec<SessionHistoryEntry>,
    pub(crate) rollout_path: Option<PathBuf>,
    pub(crate) session_approval_cache: ApprovalGrantCache,
    pub(crate) turn_approval_cache: ApprovalGrantCache,
    pub(crate) summary: RuntimeSessionSummary,
    pub(crate) active_turn_usage: Option<TurnUsage>,
    pub(crate) collaboration_mode: CollaborationMode,
    pub(crate) hook_context: HookContextSnapshot,
    pub(crate) latest_compaction_snapshot: Option<CompactionSnapshotLine>,
    /// Live sandbox profile shared with tool execution (L2-DES-CONV-002
    /// Phase 3): the settings override path mutates it mid-turn and the tool
    /// router reads it per spawn. `hook_context.config.sandbox_profile` is
    /// updated alongside so admission checks see the same value.
    pub(crate) sandbox_profile_live: Arc<std::sync::Mutex<Option<String>>>,
    /// Live model/effort/compaction-limit overrides shared with the core
    /// query loop (L2-DES-CONV-002 Phase 4).
    pub(crate) live_turn_settings: devo_core::SharedLiveTurnSettings,
    /// Last assembled provider request for this turn. Auto-review clones this
    /// prefix so the reviewer shares the main-turn prompt cache.
    pub(crate) last_model_request: devo_core::SharedLastModelRequest,
    /// Wall-clock start times for in-flight items (keyed by item id). Used so
    /// completion can persist/project a distinct `created_at` for duration UI.
    pub(crate) item_started_at:
        std::collections::HashMap<NativeItemId, chrono::DateTime<chrono::Utc>>,
    /// Current transcript-tree tip for parent_id assignment on new items.
    pub(crate) transcript_leaf_id: Option<NativeItemId>,
    /// SessionLeaf write epoch mirrored from durable state.
    pub(crate) leaf_epoch: u64,
}

impl TurnInlineState {
    pub(crate) fn new(state: &SessionActorState, turn: &RuntimeTurn) -> Self {
        Self {
            turn_id: turn.native.id,
            turn_kind: turn.native.kind,
            next_item_seq: state.next_item_seq,
            loaded_item_count: state.loaded_item_count,
            persisted_turn_items: Vec::new(),
            history_items: Vec::new(),
            rollout_path: state.rollout_path.clone(),
            session_approval_cache: state.session_approval_cache.clone(),
            turn_approval_cache: state.turn_approval_cache.clone(),
            summary: state.summary.clone(),
            active_turn_usage: state
                .active_turn
                .as_ref()
                .and_then(|turn| turn.native.usage.clone()),
            collaboration_mode: state.core.collaboration_mode,
            hook_context: HookContextSnapshot {
                runtime_context: Arc::clone(&state.runtime_context),
                rollout_path: state.rollout_path.clone(),
                summary: state.summary.clone(),
                config: state.config.clone(),
            },
            latest_compaction_snapshot: None,
            sandbox_profile_live: Arc::new(std::sync::Mutex::new(
                state.config.sandbox_profile.clone(),
            )),
            live_turn_settings: Default::default(),
            last_model_request: Arc::new(std::sync::Mutex::new(None)),
            item_started_at: std::collections::HashMap::new(),
            transcript_leaf_id: state.transcript_leaf_id,
            leaf_epoch: state.leaf_epoch,
        }
    }

    pub(crate) fn allocate_item_seq(&mut self) -> u64 {
        let item_seq = self.next_item_seq;
        self.next_item_seq = self.next_item_seq.saturating_add(1);
        self.loaded_item_count = self.loaded_item_count.saturating_add(1);
        item_seq
    }

    pub(crate) fn merge_into(self, state: &mut SessionActorState) {
        state.next_item_seq = self.next_item_seq;
        state.loaded_item_count = self.loaded_item_count;
        state.persisted_turn_items.extend(self.persisted_turn_items);
        state.history_items.extend(self.history_items);
        state.session_approval_cache = self.session_approval_cache;
        state.turn_approval_cache = self.turn_approval_cache;
        state.summary.usage = self.summary.usage.clone();
        state.summary.last_query_usage = self.summary.last_query_usage.clone();
        state.summary.last_query_total_tokens = self.summary.last_query_total_tokens;
        state.core.total_input_tokens = self.summary.usage.total.input_tokens as usize;
        state.core.total_output_tokens = self.summary.usage.total.output_tokens as usize;
        state.core.total_tokens = self.summary.usage.total.total_tokens as usize;
        state.core.total_cache_creation_tokens =
            self.summary.usage.total.cache_creation_input_tokens as usize;
        state.core.total_cache_read_tokens =
            self.summary.usage.total.cache_read_input_tokens as usize;
        if let Some(snapshot) = self.latest_compaction_snapshot {
            state.latest_compaction_snapshot = Some(snapshot);
        }
        if let Some(active_turn) = state.active_turn.as_mut()
            && active_turn.native.id == self.turn_id
        {
            active_turn.native.usage = self.active_turn_usage;
        }
        state.transcript_leaf_id = self.transcript_leaf_id;
        state.leaf_epoch = self.leaf_epoch;
    }
}

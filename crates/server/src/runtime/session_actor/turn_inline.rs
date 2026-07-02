use std::sync::Arc;

use devo_core::SessionRecord;
use devo_core::TurnId;
use devo_core::TurnKind;
use devo_protocol::CollaborationMode;

use crate::execution::ApprovalGrantCache;
use crate::execution::PersistedTurnItem;
use crate::session::SessionHistoryItem;
use crate::session::SessionMetadata;
use crate::turn::TurnMetadata;

use super::SessionActorState;
use super::snapshots::HookContextSnapshot;

/// Mutable session fields updated during an in-actor turn without mailbox round-trips.
pub(crate) struct TurnInlineState {
    pub(crate) turn_id: TurnId,
    pub(crate) turn_kind: TurnKind,
    pub(crate) next_item_seq: u64,
    pub(crate) loaded_item_count: u64,
    pub(crate) persisted_turn_items: Vec<PersistedTurnItem>,
    pub(crate) history_items: Vec<SessionHistoryItem>,
    pub(crate) record: Option<SessionRecord>,
    pub(crate) session_approval_cache: ApprovalGrantCache,
    pub(crate) turn_approval_cache: ApprovalGrantCache,
    pub(crate) summary: SessionMetadata,
    pub(crate) collaboration_mode: CollaborationMode,
    pub(crate) hook_context: HookContextSnapshot,
}

impl TurnInlineState {
    pub(crate) fn new(state: &SessionActorState, turn: &TurnMetadata) -> Self {
        Self {
            turn_id: turn.turn_id,
            turn_kind: turn.kind.clone(),
            next_item_seq: state.next_item_seq,
            loaded_item_count: state.loaded_item_count,
            persisted_turn_items: Vec::new(),
            history_items: Vec::new(),
            record: state.record.clone(),
            session_approval_cache: state.session_approval_cache.clone(),
            turn_approval_cache: state.turn_approval_cache.clone(),
            summary: state.summary.clone(),
            collaboration_mode: state.core.collaboration_mode,
            hook_context: HookContextSnapshot {
                runtime_context: Arc::clone(&state.runtime_context),
                record: state.record.clone(),
                summary: state.summary.clone(),
                config: state.config.clone(),
            },
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
    }
}

use std::collections::HashMap;

use super::*;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct UsageTotals {
    pub(super) input_tokens: usize,
    pub(super) output_tokens: usize,
    pub(super) total_tokens: usize,
    pub(super) cache_creation_input_tokens: usize,
    pub(super) cache_read_input_tokens: usize,
    pub(super) reasoning_output_tokens: usize,
}

impl UsageTotals {
    pub(super) fn from_turn_usage(usage: &TurnUsage) -> Self {
        Self {
            input_tokens: usage.input_tokens as usize,
            output_tokens: usage.output_tokens as usize,
            total_tokens: usage.display_total_tokens(),
            cache_creation_input_tokens: usage.cache_creation_input_tokens.unwrap_or(0) as usize,
            cache_read_input_tokens: usage.cache_read_input_tokens.unwrap_or(0) as usize,
            reasoning_output_tokens: usage.reasoning_output_tokens.unwrap_or(0) as usize,
        }
    }

    pub(super) fn from_session_summary(summary: &SessionMetadata) -> Self {
        Self {
            input_tokens: summary.total_input_tokens,
            output_tokens: summary.total_output_tokens,
            total_tokens: summary.total_tokens,
            cache_creation_input_tokens: summary.total_cache_creation_tokens,
            cache_read_input_tokens: summary.total_cache_read_tokens,
            reasoning_output_tokens: 0,
        }
    }

    pub(super) fn to_turn_usage(self) -> TurnUsage {
        TurnUsage {
            input_tokens: saturating_u32(self.input_tokens),
            output_tokens: saturating_u32(self.output_tokens),
            cache_creation_input_tokens: nonzero_saturating_u32(self.cache_creation_input_tokens),
            cache_read_input_tokens: nonzero_saturating_u32(self.cache_read_input_tokens),
            reasoning_output_tokens: nonzero_saturating_u32(self.reasoning_output_tokens),
            total_tokens: nonzero_saturating_u32(self.total_tokens),
        }
    }

    fn add(self, other: Self) -> Self {
        Self {
            input_tokens: self.input_tokens.saturating_add(other.input_tokens),
            output_tokens: self.output_tokens.saturating_add(other.output_tokens),
            total_tokens: self.total_tokens.saturating_add(other.total_tokens),
            cache_creation_input_tokens: self
                .cache_creation_input_tokens
                .saturating_add(other.cache_creation_input_tokens),
            cache_read_input_tokens: self
                .cache_read_input_tokens
                .saturating_add(other.cache_read_input_tokens),
            reasoning_output_tokens: self
                .reasoning_output_tokens
                .saturating_add(other.reasoning_output_tokens),
        }
    }

    fn saturating_sub(self, other: Self) -> Self {
        Self {
            input_tokens: self.input_tokens.saturating_sub(other.input_tokens),
            output_tokens: self.output_tokens.saturating_sub(other.output_tokens),
            total_tokens: self.total_tokens.saturating_sub(other.total_tokens),
            cache_creation_input_tokens: self
                .cache_creation_input_tokens
                .saturating_sub(other.cache_creation_input_tokens),
            cache_read_input_tokens: self
                .cache_read_input_tokens
                .saturating_sub(other.cache_read_input_tokens),
            reasoning_output_tokens: self
                .reasoning_output_tokens
                .saturating_sub(other.reasoning_output_tokens),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ParentUsageSnapshot {
    pub(super) session_id: SessionId,
    pub(super) turn_id: TurnId,
    pub(super) turn_usage: UsageTotals,
    pub(super) session_totals: UsageTotals,
    pub(super) context_window: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
struct ParentTurnUsage {
    base_session_totals: UsageTotals,
    base_child_totals: UsageTotals,
    parent_turn_usage: UsageTotals,
    context_window: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChildUsageOwner {
    parent_session_id: SessionId,
    parent_turn_id: Option<TurnId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChildTurnUsage {
    parent_session_id: SessionId,
    parent_turn_id: Option<TurnId>,
    usage: UsageTotals,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ParentTurnKey {
    session_id: SessionId,
    turn_id: TurnId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ChildTurnKey {
    session_id: SessionId,
    turn_id: TurnId,
}

#[derive(Debug, Default)]
pub(super) struct SubagentUsageState {
    parent_turns: HashMap<ParentTurnKey, ParentTurnUsage>,
    child_owners: HashMap<SessionId, ChildUsageOwner>,
    child_turns: HashMap<ChildTurnKey, ChildTurnUsage>,
}

impl SubagentUsageState {
    pub(super) fn begin_parent_turn(
        &mut self,
        session_id: SessionId,
        turn_id: TurnId,
        base_session_totals: UsageTotals,
        context_window: Option<u64>,
    ) {
        let base_child_totals = self.child_totals_for_parent_session(session_id);
        self.parent_turns
            .entry(ParentTurnKey {
                session_id,
                turn_id,
            })
            .or_insert(ParentTurnUsage {
                base_session_totals,
                base_child_totals,
                parent_turn_usage: UsageTotals::default(),
                context_window,
            });
    }

    pub(super) fn register_child_owner(
        &mut self,
        parent_session_id: SessionId,
        child_session_id: SessionId,
        parent_turn_id: Option<TurnId>,
    ) {
        self.child_owners.insert(
            child_session_id,
            ChildUsageOwner {
                parent_session_id,
                parent_turn_id,
            },
        );
    }

    pub(super) fn record_parent_turn_usage(
        &mut self,
        session_id: SessionId,
        turn_id: TurnId,
        usage: UsageTotals,
        context_window: Option<u64>,
    ) -> Option<ParentUsageSnapshot> {
        let key = ParentTurnKey {
            session_id,
            turn_id,
        };
        let state = self.parent_turns.get_mut(&key)?;
        state.parent_turn_usage = usage;
        if context_window.is_some() {
            state.context_window = context_window;
        }
        self.snapshot_for_parent_turn(session_id, turn_id)
    }

    pub(super) fn record_child_turn_usage(
        &mut self,
        child_session_id: SessionId,
        child_turn_id: TurnId,
        usage: UsageTotals,
    ) -> Option<ParentUsageSnapshot> {
        let key = ChildTurnKey {
            session_id: child_session_id,
            turn_id: child_turn_id,
        };
        let owner = if let Some(entry) = self.child_turns.get_mut(&key) {
            entry.usage = usage;
            ChildUsageOwner {
                parent_session_id: entry.parent_session_id,
                parent_turn_id: entry.parent_turn_id,
            }
        } else {
            let owner = *self.child_owners.get(&child_session_id)?;
            self.child_turns.insert(
                key,
                ChildTurnUsage {
                    parent_session_id: owner.parent_session_id,
                    parent_turn_id: owner.parent_turn_id,
                    usage,
                },
            );
            owner
        };
        owner
            .parent_turn_id
            .and_then(|turn_id| self.snapshot_for_parent_turn(owner.parent_session_id, turn_id))
    }

    pub(super) fn snapshot_for_parent_turn(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
    ) -> Option<ParentUsageSnapshot> {
        let state = self.parent_turns.get(&ParentTurnKey {
            session_id,
            turn_id,
        })?;
        let child_turn_usage = self.child_totals_for_parent_turn(session_id, turn_id);
        let child_session_delta = self
            .child_totals_for_parent_session(session_id)
            .saturating_sub(state.base_child_totals);
        let turn_usage = state.parent_turn_usage.add(child_turn_usage);
        let session_totals = state
            .base_session_totals
            .add(state.parent_turn_usage)
            .add(child_session_delta);
        Some(ParentUsageSnapshot {
            session_id,
            turn_id,
            turn_usage,
            session_totals,
            context_window: state.context_window,
        })
    }

    fn child_totals_for_parent_session(&self, parent_session_id: SessionId) -> UsageTotals {
        self.child_turns
            .values()
            .filter(|entry| entry.parent_session_id == parent_session_id)
            .fold(UsageTotals::default(), |total, entry| {
                total.add(entry.usage)
            })
    }

    fn child_totals_for_parent_turn(
        &self,
        parent_session_id: SessionId,
        parent_turn_id: TurnId,
    ) -> UsageTotals {
        self.child_turns
            .values()
            .filter(|entry| {
                entry.parent_session_id == parent_session_id
                    && entry.parent_turn_id == Some(parent_turn_id)
            })
            .fold(UsageTotals::default(), |total, entry| {
                total.add(entry.usage)
            })
    }
}

impl ServerRuntime {
    pub(super) async fn begin_parent_usage_turn(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        context_window: Option<u64>,
    ) {
        let base_session_totals = self
            .session_summary_snapshot(session_id)
            .await
            .map(|summary| UsageTotals::from_session_summary(&summary))
            .unwrap_or_default();
        self.subagent_usage.lock().await.begin_parent_turn(
            session_id,
            turn_id,
            base_session_totals,
            context_window,
        );
    }

    pub(super) async fn register_subagent_usage_owner(
        &self,
        parent_session_id: SessionId,
        child_session_id: SessionId,
        parent_turn_id: Option<TurnId>,
    ) {
        self.subagent_usage.lock().await.register_child_owner(
            parent_session_id,
            child_session_id,
            parent_turn_id,
        );
    }

    pub(super) async fn active_turn_id_for_session(&self, session_id: SessionId) -> Option<TurnId> {
        let session_handle = self.session(session_id).await?;
        session_handle.active_turn_id().await.flatten()
    }

    pub(super) async fn publish_parent_turn_usage(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        usage: TurnUsage,
        context_window: Option<u64>,
    ) -> Option<ParentUsageSnapshot> {
        self.begin_parent_usage_turn(session_id, turn_id, context_window)
            .await;
        let snapshot = {
            let mut usage_state = self.subagent_usage.lock().await;
            usage_state.record_parent_turn_usage(
                session_id,
                turn_id,
                UsageTotals::from_turn_usage(&usage),
                context_window,
            )
        }?;
        self.apply_parent_usage_snapshot(snapshot).await;
        Some(snapshot)
    }

    pub(super) async fn publish_subagent_turn_usage(
        &self,
        child_session_id: SessionId,
        child_turn_id: TurnId,
        usage: TurnUsage,
    ) -> Option<ParentUsageSnapshot> {
        let snapshot = {
            let mut usage_state = self.subagent_usage.lock().await;
            usage_state.record_child_turn_usage(
                child_session_id,
                child_turn_id,
                UsageTotals::from_turn_usage(&usage),
            )
        }?;
        self.apply_parent_usage_snapshot(snapshot).await;
        Some(snapshot)
    }

    pub(super) async fn parent_usage_snapshot(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
    ) -> Option<ParentUsageSnapshot> {
        self.subagent_usage
            .lock()
            .await
            .snapshot_for_parent_turn(session_id, turn_id)
    }

    async fn apply_parent_usage_snapshot(&self, snapshot: ParentUsageSnapshot) {
        if let Some(session_handle) = self.session(snapshot.session_id).await {
            session_handle.apply_parent_usage_snapshot(snapshot).await;
        }
        self.broadcast_event(ServerEvent::TurnUsageUpdated(
            snapshot.to_turn_usage_updated_payload(),
        ))
        .await;
    }
}

impl ParentUsageSnapshot {
    pub(super) fn to_turn_usage_updated_payload(self) -> TurnUsageUpdatedPayload {
        TurnUsageUpdatedPayload {
            session_id: self.session_id,
            turn_id: self.turn_id,
            usage: self.turn_usage.to_turn_usage(),
            total_input_tokens: self.session_totals.input_tokens,
            total_output_tokens: self.session_totals.output_tokens,
            total_tokens: self.session_totals.total_tokens,
            total_cache_read_tokens: self.session_totals.cache_read_input_tokens,
            last_query_input_tokens: self.turn_usage.input_tokens,
            context_window: self.context_window,
        }
    }

    pub(super) fn apply_to_actor_state(
        self,
        state: &mut crate::runtime::session_actor::state::SessionActorState,
    ) {
        state.summary.total_input_tokens = self.session_totals.input_tokens;
        state.summary.total_output_tokens = self.session_totals.output_tokens;
        state.summary.total_tokens = self.session_totals.total_tokens;
        state.summary.total_cache_creation_tokens = self.session_totals.cache_creation_input_tokens;
        state.summary.total_cache_read_tokens = self.session_totals.cache_read_input_tokens;
        state.summary.last_query_total_tokens = self.turn_usage.total_tokens;
        if let Some(active_turn) = state.active_turn.as_mut()
            && active_turn.turn_id == self.turn_id
        {
            active_turn.usage = Some(self.turn_usage.to_turn_usage());
        }
        state.core.total_input_tokens = self.session_totals.input_tokens;
        state.core.total_output_tokens = self.session_totals.output_tokens;
        state.core.total_tokens = self.session_totals.total_tokens;
        state.core.total_cache_creation_tokens = self.session_totals.cache_creation_input_tokens;
        state.core.total_cache_read_tokens = self.session_totals.cache_read_input_tokens;
    }
}

fn saturating_u32(value: usize) -> u32 {
    value.try_into().unwrap_or(u32::MAX)
}

fn nonzero_saturating_u32(value: usize) -> Option<u32> {
    (value > 0).then(|| saturating_u32(value))
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn totals(input_tokens: usize, output_tokens: usize) -> UsageTotals {
        UsageTotals {
            input_tokens,
            output_tokens,
            total_tokens: input_tokens + output_tokens,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            reasoning_output_tokens: 0,
        }
    }

    #[test]
    fn child_usage_replaces_latest_value_without_double_counting() {
        let mut state = SubagentUsageState::default();
        let parent_session_id = SessionId::new();
        let parent_turn_id = TurnId::new();
        let child_session_id = SessionId::new();
        let child_turn_id = TurnId::new();

        state.begin_parent_turn(
            parent_session_id,
            parent_turn_id,
            totals(100, 10),
            Some(200_000),
        );
        state.register_child_owner(parent_session_id, child_session_id, Some(parent_turn_id));

        let parent_snapshot = state
            .record_parent_turn_usage(parent_session_id, parent_turn_id, totals(8, 2), None)
            .expect("parent snapshot");
        assert_eq!(parent_snapshot.turn_usage, totals(8, 2));
        assert_eq!(parent_snapshot.session_totals, totals(108, 12));

        let child_snapshot = state
            .record_child_turn_usage(child_session_id, child_turn_id, totals(20, 5))
            .expect("child snapshot");
        assert_eq!(child_snapshot.turn_usage, totals(28, 7));
        assert_eq!(child_snapshot.session_totals, totals(128, 17));

        let updated_child_snapshot = state
            .record_child_turn_usage(child_session_id, child_turn_id, totals(25, 6))
            .expect("updated child snapshot");
        assert_eq!(updated_child_snapshot.turn_usage, totals(33, 8));
        assert_eq!(updated_child_snapshot.session_totals, totals(133, 18));
    }

    #[test]
    fn next_parent_turn_base_includes_previous_child_usage() {
        let mut state = SubagentUsageState::default();
        let parent_session_id = SessionId::new();
        let first_parent_turn_id = TurnId::new();
        let second_parent_turn_id = TurnId::new();
        let child_session_id = SessionId::new();
        let first_child_turn_id = TurnId::new();
        let second_child_turn_id = TurnId::new();

        state.begin_parent_turn(
            parent_session_id,
            first_parent_turn_id,
            totals(100, 10),
            None,
        );
        state.register_child_owner(
            parent_session_id,
            child_session_id,
            Some(first_parent_turn_id),
        );
        state.record_child_turn_usage(child_session_id, first_child_turn_id, totals(20, 5));

        state.begin_parent_turn(
            parent_session_id,
            second_parent_turn_id,
            totals(120, 15),
            None,
        );
        state.register_child_owner(
            parent_session_id,
            child_session_id,
            Some(second_parent_turn_id),
        );
        let snapshot = state
            .record_child_turn_usage(child_session_id, second_child_turn_id, totals(7, 3))
            .expect("second turn child snapshot");

        assert_eq!(snapshot.turn_usage, totals(7, 3));
        assert_eq!(snapshot.session_totals, totals(127, 18));
    }

    #[test]
    fn existing_child_turn_keeps_original_parent_turn_owner() {
        let mut state = SubagentUsageState::default();
        let parent_session_id = SessionId::new();
        let first_parent_turn_id = TurnId::new();
        let second_parent_turn_id = TurnId::new();
        let child_session_id = SessionId::new();
        let child_turn_id = TurnId::new();

        state.begin_parent_turn(
            parent_session_id,
            first_parent_turn_id,
            totals(100, 10),
            None,
        );
        state.begin_parent_turn(
            parent_session_id,
            second_parent_turn_id,
            totals(100, 10),
            None,
        );
        state.register_child_owner(
            parent_session_id,
            child_session_id,
            Some(first_parent_turn_id),
        );
        state.record_child_turn_usage(child_session_id, child_turn_id, totals(20, 5));

        state.register_child_owner(
            parent_session_id,
            child_session_id,
            Some(second_parent_turn_id),
        );
        let snapshot = state
            .record_child_turn_usage(child_session_id, child_turn_id, totals(25, 6))
            .expect("updated child snapshot");

        assert_eq!(snapshot.turn_id, first_parent_turn_id);
        assert_eq!(snapshot.turn_usage, totals(25, 6));
        assert_eq!(snapshot.session_totals, totals(125, 16));
    }
}

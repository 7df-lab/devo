use std::collections::HashMap;

use devo_core::TurnId;
use devo_protocol::ApprovalDecisionValue;
use devo_protocol::SessionId;
use tokio::sync::Mutex;
use tokio::sync::oneshot;

use crate::execution::PendingApproval;
use crate::execution::PendingUserInput;

#[derive(Default)]
struct SessionInteractiveState {
    pending_approvals: HashMap<String, PendingApproval>,
    pending_user_inputs: HashMap<String, PendingUserInput>,
}

/// Global interactive wait lanes keyed by session id.
///
/// Approval and user-input waits are routed here so a session actor blocked in
/// `query()` never has to process mailbox messages for client responses.
#[derive(Default)]
pub(crate) struct SessionInteractiveLanes {
    inner: Mutex<HashMap<SessionId, SessionInteractiveState>>,
}

impl SessionInteractiveLanes {
    pub(crate) async fn register_pending_approval(
        &self,
        host_session_id: SessionId,
        approval_id: String,
        pending: PendingApproval,
    ) {
        self.inner
            .lock()
            .await
            .entry(host_session_id)
            .or_default()
            .pending_approvals
            .insert(approval_id, pending);
    }

    pub(crate) async fn remove_pending_approval(
        &self,
        host_session_id: SessionId,
        approval_id: &str,
    ) -> Option<PendingApproval> {
        let mut lanes = self.inner.lock().await;
        let state = lanes.get_mut(&host_session_id)?;
        let removed = state.pending_approvals.remove(approval_id);
        if state.pending_approvals.is_empty() && state.pending_user_inputs.is_empty() {
            lanes.remove(&host_session_id);
        }
        removed
    }

    pub(crate) async fn register_pending_user_input(
        &self,
        session_id: SessionId,
        request_id: String,
        pending: PendingUserInput,
    ) {
        self.inner
            .lock()
            .await
            .entry(session_id)
            .or_default()
            .pending_user_inputs
            .insert(request_id, pending);
    }

    pub(crate) async fn take_pending_user_input(
        &self,
        session_id: SessionId,
        request_id: &str,
        expected_turn_id: TurnId,
    ) -> Result<PendingUserInput, UserInputTakeError> {
        let mut lanes = self.inner.lock().await;
        let Some(state) = lanes.get_mut(&session_id) else {
            return Err(UserInputTakeError::NotFound);
        };
        let Some(pending) = state.pending_user_inputs.remove(request_id) else {
            return Err(UserInputTakeError::NotFound);
        };
        if pending.turn_id != expected_turn_id {
            state
                .pending_user_inputs
                .insert(request_id.to_string(), pending);
            return Err(UserInputTakeError::WrongTurn);
        }
        if state.pending_approvals.is_empty() && state.pending_user_inputs.is_empty() {
            lanes.remove(&session_id);
        }
        Ok(pending)
    }

    pub(crate) async fn has_pending_interactive(&self, session_id: SessionId) -> bool {
        self.inner
            .lock()
            .await
            .get(&session_id)
            .is_some_and(|state| {
                !state.pending_approvals.is_empty() || !state.pending_user_inputs.is_empty()
            })
    }

    pub(crate) async fn has_pending_approval_for_session(
        &self,
        host_session_id: SessionId,
        owner_session_id: SessionId,
    ) -> bool {
        self.inner
            .lock()
            .await
            .get(&host_session_id)
            .is_some_and(|state| {
                state
                    .pending_approvals
                    .values()
                    .any(|pending| pending.owner_session_id == owner_session_id)
            })
    }

    pub(crate) async fn clear_pending_user_inputs_for_turn(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
    ) -> usize {
        let mut lanes = self.inner.lock().await;
        let Some(state) = lanes.get_mut(&session_id) else {
            return 0;
        };
        let previous_len = state.pending_user_inputs.len();
        state
            .pending_user_inputs
            .retain(|_, pending| pending.turn_id != turn_id);
        let removed_len = previous_len.saturating_sub(state.pending_user_inputs.len());
        if state.pending_approvals.is_empty() && state.pending_user_inputs.is_empty() {
            lanes.remove(&session_id);
        }
        removed_len
    }

    pub(crate) async fn clear_session(&self, session_id: SessionId) {
        self.inner.lock().await.remove(&session_id);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UserInputTakeError {
    NotFound,
    WrongTurn,
}

pub(crate) async fn complete_approval_wait(
    rx: oneshot::Receiver<ApprovalDecisionValue>,
) -> Result<ApprovalDecisionValue, String> {
    match rx.await {
        Ok(ApprovalDecisionValue::Approve) => Ok(ApprovalDecisionValue::Approve),
        Ok(ApprovalDecisionValue::Deny) => Err("rejected by user".to_string()),
        Ok(ApprovalDecisionValue::Cancel) => Err("cancelled by user".to_string()),
        Err(_) => Err("approval channel closed".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[tokio::test]
    async fn pending_approval_tracks_the_originating_child_session() {
        let lanes = SessionInteractiveLanes::default();
        let parent_session_id = SessionId::new();
        let child_session_id = SessionId::new();
        let (tx, _rx) = oneshot::channel();
        lanes
            .register_pending_approval(
                parent_session_id,
                "approval-1".to_string(),
                PendingApproval {
                    owner_session_id: child_session_id,
                    tool_name: "exec_command".to_string(),
                    path: None,
                    host: None,
                    command_prefix: None,
                    tx,
                },
            )
            .await;

        assert_eq!(
            lanes
                .has_pending_approval_for_session(parent_session_id, child_session_id)
                .await,
            true
        );
        assert_eq!(
            lanes
                .has_pending_approval_for_session(parent_session_id, parent_session_id)
                .await,
            false
        );
    }
}

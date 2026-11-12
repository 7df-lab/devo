//! Kernel goal controls share the Native lifecycle implementation.
use super::*;

#[derive(Clone, Copy)]
pub(super) enum GoalTransitionOrigin {
    Client,
    Kernel,
}

impl ServerRuntime {
    pub(super) async fn stop_goal_continuation_for_transition(
        self: &Arc<Self>,
        session_id: SessionId,
        reason: &str,
        origin: GoalTransitionOrigin,
    ) {
        match origin {
            GoalTransitionOrigin::Client => {
                self.interrupt_active_goal_continuation_turn(session_id, reason)
                    .await;
            }
            GoalTransitionOrigin::Kernel => {
                // Do not interrupt this host reply, but retain turn ownership:
                // a later client control must still be able to interrupt it,
                // including after Python pause -> resume in the same cell.
                self.cancel_background_goal_wait(session_id);
            }
        }
    }

    pub(crate) async fn host_goal_transition(
        self: &Arc<Self>,
        session_id: &str,
        action: &str,
    ) -> serde_json::Value {
        let method = match action {
            "goal.pause" => "session/goal/pause",
            "goal.resume" => "session/goal/resume",
            "goal.cancel" => "session/goal/cancel",
            _ => return serde_json::json!({"status": "error", "error": "unknown goal action"}),
        };
        let sid = SessionId::from_string(session_id.to_owned());
        let goal_id = self
            .goal_stores
            .lock()
            .await
            .get(&sid)
            .and_then(GoalStore::get)
            .map(|goal| goal.goal_id);
        let Some(goal_id) = goal_id else {
            return serde_json::json!({"status": "error", "error": "session has no goal"});
        };
        let response = self
            .handle_session_goal_transition(
                method,
                serde_json::Value::Null,
                serde_json::json!({"sessionId": sid, "expectedGoalId": goal_id}),
                GoalTransitionOrigin::Kernel,
            )
            .await;
        if let Some(error) = response.get("error") {
            return serde_json::json!({"status": "error", "error": error.get("message").unwrap_or(error)});
        }
        serde_json::json!({"status": "ok", "goal": response["result"]["goal"]})
    }
}

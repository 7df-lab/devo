use std::sync::Arc;
use std::time::Duration;

use devo_protocol::native::ids::{SessionId, TurnId};
use devo_protocol::native::turn::Turn;
use pretty_assertions::assert_eq;
use tokio_util::sync::CancellationToken;

use crate::test_support::{NoopProvider, TestRuntime};

#[tokio::test]
async fn concurrent_deletion_aborts_turns_that_ignore_cooperative_cancellation() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::new(Arc::new(NoopProvider::default())).runtime(directory.path());
    let mut executions = Vec::new();
    for _ in 0..12 {
        let session_id = SessionId::new();
        let turn: Turn = serde_json::from_value(serde_json::json!({
            "id": TurnId::new(), "sessionId": session_id, "sequence": 1,
            "kind": "regular", "status": "inProgress",
            "model": { "provider": "test", "model": "test-model" },
            "startedAt": chrono::Utc::now(),
        }))
        .unwrap();
        let token = CancellationToken::new();
        let task = tokio::spawn(std::future::pending::<()>());
        let terminal = runtime.subscribe_terminal_turn_status(turn.id).await;
        runtime.active_turns.register_turn(session_id, turn).await;
        runtime
            .active_turns
            .insert_cancel_token(session_id, token.clone())
            .await;
        runtime
            .active_turns
            .set_abort_handle(session_id, task.abort_handle())
            .await;
        executions.push((session_id, token, task, terminal));
    }
    // No task observes its cancellation token or emits a terminal event. The
    // fallback must abort all of them after one shared, bounded grace period.
    tokio::time::timeout(
        Duration::from_secs(2),
        futures::future::join_all(
            executions
                .iter()
                .map(|(session_id, _, _, _)| runtime.delete_session_tree(*session_id)),
        ),
    )
    .await
    .expect("deletion must not wait five seconds for each blocked turn")
    .into_iter()
    .for_each(|result| assert_eq!(result.unwrap(), Vec::<SessionId>::new()));
    let mut results = Vec::new();
    let mut terminals = Vec::new();
    for (session_id, token, task, terminal) in executions {
        results.push((
            token.is_cancelled(),
            task.await.unwrap_err().is_cancelled(),
            runtime.active_turns.active_turn_id(session_id).await,
        ));
        terminals.push(terminal.await.unwrap());
    }
    assert_eq!(results, vec![(true, true, None); 12]);
    assert_eq!(
        terminals,
        vec![
            super::TerminalTurnSnapshot {
                status: devo_protocol::TurnStatus::Interrupted,
                stop_reason: None,
                failure_reason: None,
            };
            12
        ]
    );
    assert!(runtime.acp_prompt_waiters.lock().await.is_empty());
}

use std::sync::Arc;
use std::time::Duration;

use devo_protocol::native::ids::{SessionId, TurnId};
use devo_protocol::native::turn::Turn;
use pretty_assertions::assert_eq;
use tokio_util::sync::CancellationToken;

use crate::runtime::connection::ConnectionProtocol;
use crate::runtime::outbound::test_outbound_channel;
use crate::test_support::{NoopProvider, TestRuntime};

#[tokio::test]
async fn concurrent_deletion_aborts_turns_that_ignore_cooperative_cancellation() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::new(Arc::new(NoopProvider::default())).runtime(directory.path());
    let (outbound, mut notifications) = test_outbound_channel(/*capacity*/ 32);
    let connection_id = runtime
        .register_connection(crate::ClientTransportKind::Stdio, outbound)
        .await;
    runtime
        .connections
        .lock()
        .await
        .get_mut(&connection_id)
        .unwrap()
        .protocol = Some(ConnectionProtocol::Native);
    let mut executions = Vec::new();
    let mut expected_turns = std::collections::HashMap::new();
    for _ in 0..12 {
        let session_id = SessionId::new();
        let turn: Turn = serde_json::from_value(serde_json::json!({
            "id": TurnId::new(), "sessionId": session_id, "sequence": 1,
            "kind": "regular", "status": "inProgress",
            "model": { "provider": "test", "model": "test-model" },
            "startedAt": chrono::Utc::now(),
        }))
        .unwrap();
        expected_turns.insert(turn.id, turn.clone());
        runtime
            .connections
            .lock()
            .await
            .get_mut(&connection_id)
            .unwrap()
            .event_selectors
            .push(devo_protocol::native::event::StreamSelector::Session { session_id });
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
    tokio::time::timeout(Duration::from_secs(/*secs*/ 2), async {
        for _ in 0..12 {
            let notification = notifications.recv().await.expect("terminal notification");
            assert_eq!(notification["method"], "turn/completed");
            let actual: Turn =
                serde_json::from_value(notification["params"]["turn"].clone()).unwrap();
            let mut expected = expected_turns
                .remove(&actual.id)
                .expect("exactly one event per aborted turn");
            expected.status = devo_protocol::native::turn::TurnStatus::Interrupted;
            expected.completed_at = Some(
                actual
                    .completed_at
                    .expect("interrupted turn completion time"),
            );
            assert_eq!(actual, expected);
        }
    })
    .await
    .expect("all aborted turns must notify clients before deletion returns");
    assert!(expected_turns.is_empty());
    assert!(notifications.try_recv().is_err());
}

#[tokio::test]
async fn lifecycle_admission_guards_are_drop_safe_and_mutually_exclusive() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::new(Arc::new(NoopProvider::default())).runtime(directory.path());
    let root_id = SessionId::new();

    let operation = runtime
        .begin_lifecycle_operation_for_root(root_id)
        .expect("spawn/resume reservation");
    assert!(
        runtime.begin_delete_admission_for_root(root_id).is_err(),
        "delete must not start while spawn/resume reservation is active"
    );
    drop(operation);

    let delete = runtime
        .begin_delete_admission_for_root(root_id)
        .expect("delete admission after reservation drop");
    assert!(
        runtime.begin_lifecycle_operation_for_root(root_id).is_err(),
        "spawn/resume must not start while delete marker is active"
    );
    drop(delete);

    runtime
        .begin_lifecycle_operation_for_root(root_id)
        .expect("aborted/dropped delete must release admission");
}

#[tokio::test]
async fn lifecycle_root_cycle_fails_without_delete_mutation() {
    use crate::db::SessionIndexRow;
    use std::path::PathBuf;

    let directory = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::new(Arc::new(NoopProvider::default())).runtime(directory.path());
    let first = SessionId::new();
    let second = SessionId::new();
    let now = chrono::Utc::now();
    let row = |id: SessionId, parent: SessionId| SessionIndexRow {
        session_id: id,
        cwd: PathBuf::from(directory.path()),
        additional_directories: Vec::new(),
        created_at: now,
        updated_at: now,
        last_activity_at: now,
        title: Some("cycle".into()),
        title_state: devo_core::SessionTitleState::Unset,
        parent_session_id: Some(parent),
        fork_from_id: None,
        fork_at_turn_id: None,
        agent_path: Some("subagent".into()),
        ephemeral: false,
        model: Some("test-model".into()),
        reasoning_effort_selection: None,
    };
    runtime
        .deps
        .db
        .upsert_session(row(first, second), None)
        .expect("first row");
    runtime
        .deps
        .db
        .upsert_session(row(second, first), None)
        .expect("second row");

    let error = runtime
        .resolve_lifecycle_root(first, None)
        .await
        .expect_err("cycle must fail");
    assert!(
        matches!(error, super::LifecycleRootError::Cycle { .. }),
        "unexpected error: {error}"
    );
    let result = runtime.delete_session_tree(first).await;
    assert!(result.is_err(), "cycle delete must fail closed");
    assert!(
        runtime.deps.db.get_session_index(&first).unwrap().is_some(),
        "failed delete must not mutate DB rows"
    );
}

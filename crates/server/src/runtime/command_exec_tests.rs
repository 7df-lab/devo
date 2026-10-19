use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn terminal_snapshots_wait_for_retained_output_and_do_not_duplicate_live_entries() {
    let manager = CommandExecManager::new();
    let session_id = SessionId::from_string("ses_exec_output_completion".to_string());
    let key = CommandExecKey {
        connection_id: 1,
        session_id: Some(session_id),
        process_id: "process-output-completion".to_string(),
    };
    let command = "interactive fixture".to_string();
    manager.sessions.lock().await.insert(
        key.clone(),
        CommandExecSession {
            store_process_id: 1,
            command: command.clone(),
        },
    );
    manager
        .tails
        .lock()
        .await
        .insert(key.clone(), b"ready\n".to_vec());

    // An exited/absent OS process is insufficient: the output pump has not yet
    // committed its terminal record. Reads must keep the task running.
    assert_eq!(
        manager.task_snapshot(&key.process_id).await,
        Some(TaskProcessSnapshot {
            process_id: key.process_id.clone(),
            session_id: Some(session_id),
            command: command.clone(),
            is_running: true,
            exit_code: None,
            tail: b"ready\n".to_vec(),
        })
    );

    let tail = b"ready\nreceived:hello\ndone\n".to_vec();
    manager
        .completed
        .lock()
        .await
        .push_back(CompletedTaskRecord {
            session_id: Some(session_id),
            process_id: key.process_id.clone(),
            command: command.clone(),
            exit_code: Some(0),
            tail: tail.clone(),
        });
    let expected = TaskProcessSnapshot {
        process_id: key.process_id.clone(),
        session_id: Some(session_id),
        command,
        is_running: false,
        exit_code: Some(0),
        tail,
    };

    // The live entry and old tail can remain during persistence. Both APIs
    // must prefer the complete retained record and list it only once.
    assert_eq!(
        manager.task_snapshot(&key.process_id).await,
        Some(expected.clone())
    );
    assert_eq!(
        manager.task_snapshots_for_session(session_id).await,
        vec![expected]
    );
}

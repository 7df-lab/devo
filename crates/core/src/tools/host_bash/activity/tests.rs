use super::*;
use crate::tools::host_bash::BashMode;
use crate::tools::host_bash::{cancel_session, control};
use pretty_assertions::assert_eq;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

fn insert_entry(session: &str, id: &str) -> Arc<Mutex<Entry>> {
    let entry = Arc::new(Mutex::new(Entry {
        mode: BashMode::Pipe,
        io: None,
        session_id: session.into(),
        command: "echo fixture".into(),
        turn_id: None,
        cancelled: false,
        cwd: PathBuf::from("workspace"),
        created_at: chrono::Utc::now(),
        cancel: CancellationToken::new(),
        expiry_cancel: CancellationToken::new(),
        started: Instant::now(),
        finished: None,
        output: String::new(),
        tail: crate::tools::host_bash::OutputTail::default(),
        output_truncated: false,
        exit_code: None,
        error: None,
    }));
    HANDLES
        .lock()
        .unwrap()
        .insert(id.into(), Arc::clone(&entry));
    entry
}

#[test]
fn activity_is_session_scoped_and_remains_readable_after_forget() {
    let session = Uuid::new_v4().to_string();
    let other_session = Uuid::new_v4().to_string();
    let id = Uuid::new_v4().to_string();
    let other_id = Uuid::new_v4().to_string();
    let entry = insert_entry(&session, &id);
    let other = insert_entry(&other_session, &other_id);
    let live = HostBashActivity::from_entry(&id, &entry.lock().unwrap());
    let current = session_activities(&session);
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].handle_id, live.handle_id);
    assert!(current[0].running);
    {
        let mut entry = entry.lock().unwrap();
        entry.finished = Some(Instant::now());
        entry.exit_code = Some(0);
        entry.output = "done\n".into();
    }
    let terminal = HostBashActivity::from_entry(&id, &entry.lock().unwrap());
    assert_eq!(
        control(
            &session,
            "bash.forget",
            &serde_json::json!({"handle_id":id})
        ),
        serde_json::json!({"status":"ok","result":{"forgotten":true}})
    );
    assert_eq!(session_activities(&session), vec![terminal.clone()]);
    assert_eq!(activity_snapshot(&id), Some(terminal));
    cancel_session(&session);
    assert_eq!(session_activities(&session), vec![]);
    assert_eq!(activity_snapshot(&id), None);
    assert!(!other.lock().unwrap().cancel.is_cancelled());
    cancel_session(&other_session);
}

#[test]
fn activity_output_is_utf8_bounded_and_preserves_errors() {
    let session = Uuid::new_v4().to_string();
    let id = Uuid::new_v4().to_string();
    let entry = insert_entry(&session, &id);
    {
        let mut entry = entry.lock().unwrap();
        entry.finished = Some(Instant::now());
        entry.output = "中".repeat(OUTPUT_LIMIT);
        entry.tail.append(b"raw process output without diagnostics");
        entry.error = Some("failed".into());
    }
    let result = activity_snapshot(&id).unwrap();
    assert!(result.output.len() <= OUTPUT_LIMIT);
    assert_eq!(result.output, "中".repeat(OUTPUT_LIMIT / 3));
    assert_eq!(
        (result.running, result.exit_code, result.error),
        (false, None, Some("failed".into()))
    );
    cancel_session(&session);
}

#[test]
fn expired_completed_observations_are_not_returned() {
    let session = Uuid::new_v4().to_string();
    let id = Uuid::new_v4().to_string();
    let entry = insert_entry(&session, &id);
    {
        let mut entry = entry.lock().unwrap();
        entry.started = Instant::now() - RETENTION - std::time::Duration::from_secs(1);
        entry.finished = Some(entry.started);
        entry.exit_code = Some(0);
    }
    control(
        &session,
        "bash.forget",
        &serde_json::json!({"handle_id":id}),
    );
    assert_eq!(session_activities(&session), vec![]);
    assert_eq!(activity_snapshot(&id), None);
    cancel_session(&session);
}

#[test]
fn activity_uses_recent_tail_for_successful_output() {
    let session = Uuid::new_v4().to_string();
    let id = Uuid::new_v4().to_string();
    let entry = insert_entry(&session, &id);
    {
        let mut entry = entry.lock().unwrap();
        entry.output = "stale prefix".into();
        entry.tail.append(b"latest output");
    }
    assert_eq!(activity_snapshot(&id).unwrap().output, "latest output");
    cancel_session(&session);
}

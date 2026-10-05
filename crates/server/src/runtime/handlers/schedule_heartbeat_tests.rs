use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use pretty_assertions::assert_eq;
use tempfile::TempDir;

use crate::SuccessResponse;
use crate::test_support::TestRuntime;

#[tokio::test]
async fn heartbeat_pause_and_clear_wait_for_dispatch_before_mutating_due_job() -> Result<()> {
    use devo_protocol::native::rpc_schedule::{
        HeartbeatCommandAction, ScheduleDeliveryMode, ScheduleJobKind, ScheduleJobStatus,
        SessionHeartbeatCommandResult, SessionScheduleUpsertParams,
    };

    let data_root = TempDir::new()?;
    let runtime = TestRuntime::new(Arc::new(crate::test_support::NoopProvider::failing()))
        .runtime(data_root.path());
    let session_id = devo_protocol::native::ids::SessionId::new();
    let job = runtime.schedule_store.upsert(SessionScheduleUpsertParams {
        kind: ScheduleJobKind::Heartbeat,
        session_id,
        job_id: None,
        cwd: None,
        schedule: Some("every 1h".into()),
        interval_ms: None,
        prompt: None,
        instruction: Some("check pending work".into()),
        delivery_mode: ScheduleDeliveryMode::FollowUp,
        label: None,
    })?;
    runtime
        .schedule_store
        .claim_due_with_outbox(chrono::Utc::now() + chrono::Duration::hours(2))?;
    assert_eq!(
        runtime
            .schedule_store
            .pending_occurrences(chrono::Utc::now())?
            .len(),
        1
    );

    for (args, action, status) in [
        (
            "pause",
            HeartbeatCommandAction::Pause,
            Some(ScheduleJobStatus::Paused),
        ),
        ("clear", HeartbeatCommandAction::Clear, None),
    ] {
        let before = runtime.schedule_store.get_job(&job.job_id)?;
        let dispatch_guard = runtime.schedule_store.lock_dispatch().await;
        let command_runtime = Arc::clone(&runtime);
        let mut command = tokio::spawn(async move {
            command_runtime
                .handle_native_session_heartbeat_command(
                    serde_json::json!(1),
                    serde_json::json!({ "sessionId": session_id, "args": args }),
                )
                .await
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut command)
                .await
                .is_err(),
            "heartbeat {args} must wait for an in-progress dispatch"
        );
        assert_eq!(runtime.schedule_store.get_job(&job.job_id)?, before);
        drop(dispatch_guard);
        let response: SuccessResponse<SessionHeartbeatCommandResult> =
            serde_json::from_value(command.await.context("heartbeat command task")?)?;
        assert_eq!(response.result.action, action);
        assert_eq!(
            runtime
                .schedule_store
                .get_job(&job.job_id)?
                .map(|job| job.status),
            status
        );
    }

    // A claimed occurrence must not be admitted after pause/clear return.
    runtime.dispatch_due_schedule_jobs().await?;
    assert_eq!(
        runtime
            .schedule_store
            .pending_occurrences(chrono::Utc::now())?,
        vec![]
    );
    Ok(())
}

#[tokio::test]
async fn rlm_heartbeat_mutations_wait_for_dispatch_lock() -> Result<()> {
    use devo_protocol::native::rpc_schedule::ScheduleJobStatus;

    let data_root = TempDir::new()?;
    let runtime = TestRuntime::new(Arc::new(crate::test_support::NoopProvider::failing()))
        .runtime(data_root.path());
    let session_id = devo_protocol::native::ids::SessionId::new();
    let dispatch_guard = runtime.schedule_store.lock_dispatch().await;
    let create_runtime = Arc::clone(&runtime);
    let mut create = tokio::spawn(async move {
        create_runtime
            .host_rlm_heartbeat_create(
                session_id.as_str(),
                &serde_json::json!({ "instruction": "check work", "interval": "1h" }),
            )
            .await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut create)
            .await
            .is_err(),
        "RLM heartbeat create must wait for dispatch"
    );
    assert_eq!(
        runtime.schedule_store.list(Some(&session_id), None)?,
        vec![]
    );
    drop(dispatch_guard);
    let created = create.await.context("RLM heartbeat create task")?;
    assert_eq!(created["status"], "ok");
    let job_id = devo_protocol::native::ids::JobId::from_string(
        created["result"]["id"]
            .as_str()
            .context("created heartbeat id")?
            .to_string(),
    );

    for (params, expected_status) in [
        (
            serde_json::json!({ "id": job_id, "status": "pause" }),
            ScheduleJobStatus::Paused,
        ),
        (
            serde_json::json!({ "id": job_id, "instruction": "new work" }),
            ScheduleJobStatus::Paused,
        ),
    ] {
        let before = runtime.schedule_store.get_job(&job_id)?;
        let dispatch_guard = runtime.schedule_store.lock_dispatch().await;
        let update_runtime = Arc::clone(&runtime);
        let mut update = tokio::spawn(async move {
            update_runtime
                .host_rlm_heartbeat_update(session_id.as_str(), &params)
                .await
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut update)
                .await
                .is_err(),
            "RLM heartbeat update must wait for dispatch"
        );
        assert_eq!(runtime.schedule_store.get_job(&job_id)?, before);
        drop(dispatch_guard);
        let updated = update.await.context("RLM heartbeat update task")?;
        assert_eq!(updated["status"], "ok");
        assert_eq!(
            runtime
                .schedule_store
                .get_job(&job_id)?
                .map(|job| job.status),
            Some(expected_status)
        );
    }

    let before = runtime.schedule_store.get_job(&job_id)?;
    let dispatch_guard = runtime.schedule_store.lock_dispatch().await;
    let delete_runtime = Arc::clone(&runtime);
    let mut delete = tokio::spawn(async move {
        delete_runtime
            .host_rlm_heartbeat_delete(session_id.as_str(), &serde_json::json!({ "id": job_id }))
            .await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut delete)
            .await
            .is_err(),
        "RLM heartbeat delete must wait for dispatch"
    );
    assert_eq!(runtime.schedule_store.get_job(&job_id)?, before);
    drop(dispatch_guard);
    let deleted = delete.await.context("RLM heartbeat delete task")?;
    assert_eq!(deleted["status"], "ok");
    assert_eq!(runtime.schedule_store.get_job(&job_id)?, None);
    Ok(())
}

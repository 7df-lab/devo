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
async fn rlm_schedule_mutations_wait_for_dispatch_lock() -> Result<()> {
    use crate::runtime::kernel_host::dispatch_host_request_with_bridge;
    use crate::runtime::kernel_host_bridge::HostBridge;
    use devo_protocol::native::rpc_schedule::ScheduleJobStatus;
    use serde_json::json;

    let data_root = TempDir::new()?;
    let runtime = TestRuntime::new(Arc::new(crate::test_support::NoopProvider::failing()))
        .runtime(data_root.path());
    let session_id = devo_protocol::native::ids::SessionId::new();
    let mut bridge = HostBridge::for_tests(session_id);
    bridge.runtime = Arc::downgrade(&runtime);
    let bridge = Arc::new(bridge);
    let dispatch_guard = runtime.schedule_store.lock_dispatch().await;
    let create_bridge = Arc::clone(&bridge);
    let mut create = tokio::spawn(async move {
        dispatch_host_request_with_bridge(
            &create_bridge,
            json!({
                "action": "rlm_schedule.create",
                "params": { "instruction": "check work", "interval": "1h", "label": "work" }
            }),
        )
        .await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut create)
            .await
            .is_err(),
        "RLM schedule create must wait for dispatch"
    );
    assert_eq!(
        runtime.schedule_store.list(Some(&session_id), None)?,
        vec![]
    );
    drop(dispatch_guard);
    let created = create.await.context("RLM schedule create task")?;
    assert_eq!(created["status"], "ok");
    assert_eq!(
        dispatch_host_request_with_bridge(&bridge, json!({ "action": "rlm_schedule.list" })).await,
        json!({ "status": "ok", "result": { "schedules": [created["result"]] } })
    );
    let job_id = devo_protocol::native::ids::JobId::from_string(
        created["result"]["id"]
            .as_str()
            .context("created scheduled check id")?
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
        let update_bridge = Arc::clone(&bridge);
        let mut update = tokio::spawn(async move {
            dispatch_host_request_with_bridge(
                &update_bridge,
                json!({ "action": "rlm_schedule.update", "params": params }),
            )
            .await
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut update)
                .await
                .is_err(),
            "RLM schedule update must wait for dispatch"
        );
        assert_eq!(runtime.schedule_store.get_job(&job_id)?, before);
        drop(dispatch_guard);
        let updated = update.await.context("RLM schedule update task")?;
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
    let mut delete = tokio::spawn(async move {
        dispatch_host_request_with_bridge(
            &bridge,
            json!({ "action": "rlm_schedule.delete", "params": { "id": job_id } }),
        )
        .await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut delete)
            .await
            .is_err(),
        "RLM schedule delete must wait for dispatch"
    );
    assert_eq!(runtime.schedule_store.get_job(&job_id)?, before);
    drop(dispatch_guard);
    let deleted = delete.await.context("RLM schedule delete task")?;
    assert_eq!(deleted["status"], "ok");
    assert_eq!(runtime.schedule_store.get_job(&job_id)?, None);
    Ok(())
}

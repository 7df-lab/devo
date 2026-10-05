use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use devo_core::{PendingInputItem, PendingInputKind};
use devo_protocol::{ModelRequest, ModelResponse, StreamEvent};
use devo_provider::ModelProviderSDK;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use uuid::Uuid;

use crate::ClientTransportKind;
use crate::SuccessResponse;
use crate::runtime::outbound::test_outbound_channel;
use crate::test_support::TestRuntime;

struct GatedProvider {
    open: Arc<AtomicBool>,
    started: Arc<AtomicBool>,
}

#[async_trait]
impl ModelProviderSDK for GatedProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        anyhow::bail!("gated provider does not support completion")
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        self.started.store(true, Ordering::SeqCst);
        let open = Arc::clone(&self.open);
        Ok(Box::pin(futures::stream::unfold(false, move |done| {
            let open = Arc::clone(&open);
            async move {
                if done {
                    return None;
                }
                let gate_open = async {
                    while !open.load(Ordering::SeqCst) {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                };
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {
                        Some((
                            Ok(StreamEvent::TextDelta {
                                index: 0,
                                text: "tick".into(),
                            }),
                            false,
                        ))
                    }
                    _ = gate_open => {
                        Some((
                            Ok(StreamEvent::MessageDone {
                                response: ModelResponse {
                                    id: "schedule-gated-response".into(),
                                    content: vec![devo_protocol::ResponseContent::Text("done".into())],
                                    stop_reason: Some(devo_protocol::StopReason::EndTurn),
                                    usage: devo_protocol::Usage::default(),
                                    metadata: devo_protocol::ResponseMetadata::default(),
                                },
                            }),
                            true,
                        ))
                    }
                }
            }
        })))
    }

    fn name(&self) -> &str {
        "schedule-gated-provider"
    }
}

fn queue_has_prompt(queue: &StdMutex<VecDeque<PendingInputItem>>, prompt: &str) -> bool {
    queue
        .lock()
        .expect("pending input queue mutex should not be poisoned")
        .iter()
        .any(|item| match &item.kind {
            PendingInputKind::UserText { text } => text == prompt,
            PendingInputKind::UserInput { display_text, .. } => display_text == prompt,
            _ => false,
        })
}

async fn wait_for_provider_start(started: &AtomicBool) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !started.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("model provider did not start")
}

async fn scheduled_message_count(
    runtime: &crate::runtime::ServerRuntime,
    session_id: devo_protocol::native::ids::SessionId,
    client_user_message_id: &str,
) -> Result<usize> {
    let mut count = 0;
    for queue_type in [crate::db::QueueType::Turn, crate::db::QueueType::Steer] {
        count += runtime
            .deps
            .db
            .list_pending(&session_id, queue_type)?
            .iter()
            .filter(|item| {
                item.metadata.as_ref().is_some_and(|metadata| {
                    metadata
                        .get("clientUserMessageId")
                        .and_then(serde_json::Value::as_str)
                        == Some(client_user_message_id)
                })
            })
            .count();
    }
    if let Some(rollout_path) = runtime.resolve_rollout_path(&session_id).await {
        let history = devo_core::read_canonical_history(&rollout_path)?;
        count += history
            .items
            .iter()
            .filter(|envelope| {
                matches!(
                    &envelope.item,
                    devo_protocol::native::item::Item::UserMessage {
                        client_user_message_id: Some(id),
                        ..
                    } if id == client_user_message_id
                )
            })
            .count();
    }
    Ok(count)
}

#[tokio::test]
async fn active_schedule_steer_injects_while_follow_up_queues() -> Result<()> {
    let open = Arc::new(AtomicBool::new(false));
    let started = Arc::new(AtomicBool::new(false));
    let data_root = TempDir::new()?;
    let runtime = TestRuntime::new(Arc::new(GatedProvider {
        open: Arc::clone(&open),
        started: Arc::clone(&started),
    }))
    .db_file("schedule.db")
    .runtime(data_root.path());
    let (outbound, _receiver) = test_outbound_channel(16);
    let connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, outbound)
        .await;
    runtime
        .handle_acp_initialize(
            connection_id,
            Some(serde_json::json!(1)),
            serde_json::json!({
                "protocolVersion": 1,
                "clientCapabilities": { "terminal": false },
                "_meta": { "devo": { "protocol": "native" } },
            }),
        )
        .await;

    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 2,
                "method": "session/new",
                "params": {
                    "cwd": data_root.path(),
                    "idempotencyKey": format!("schedule-session-{}", Uuid::new_v4()),
                },
            }),
        )
        .await
        .context("session/new response")?;
    let session: SuccessResponse<devo_protocol::native::rpc_session::SessionNewResult> =
        serde_json::from_value(response)?;
    let session_id = session.result.session.id;

    runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 3,
                "method": "turn/start",
                "params": {
                    "sessionId": session_id,
                    "input": [{ "type": "text", "text": "hold this turn" }],
                    "idempotencyKey": format!("schedule-turn-{}", Uuid::new_v4()),
                },
            }),
        )
        .await
        .context("turn/start response")?;
    wait_for_provider_start(&started).await?;

    let mut initial_job_ids = Vec::new();
    for (label, delivery_mode, instruction) in [
        (
            "scheduled-steer",
            devo_protocol::native::rpc_schedule::ScheduleDeliveryMode::Steer,
            "scheduled steer",
        ),
        (
            "scheduled-follow-up",
            devo_protocol::native::rpc_schedule::ScheduleDeliveryMode::FollowUp,
            "scheduled follow-up",
        ),
        (
            "duplicate-active-steer",
            devo_protocol::native::rpc_schedule::ScheduleDeliveryMode::Steer,
            "hold this turn",
        ),
        (
            "duplicate-active-follow-up",
            devo_protocol::native::rpc_schedule::ScheduleDeliveryMode::FollowUp,
            "hold this turn",
        ),
    ] {
        let job = runtime.schedule_store.upsert(
            devo_protocol::native::rpc_schedule::SessionScheduleUpsertParams {
                kind: devo_protocol::native::rpc_schedule::ScheduleJobKind::Heartbeat,
                session_id,
                job_id: None,
                cwd: None,
                schedule: Some("every 1s".into()),
                interval_ms: None,
                prompt: None,
                instruction: Some(instruction.into()),
                delivery_mode,
                label: Some(label.into()),
            },
        )?;
        initial_job_ids.push(job.job_id);
    }
    tokio::time::sleep(Duration::from_millis(1_050)).await;
    runtime.dispatch_due_schedule_jobs().await?;

    let reservation = runtime
        .session_turn_reservation_snapshot(session_id)
        .await
        .context("active turn reservation")?;
    let active_turn = reservation
        .active_turn
        .as_ref()
        .expect("gated turn must remain active");
    assert!(
        runtime
            .active_turn_user_texts(session_id, active_turn.turn_id())
            .await
            .iter()
            .any(|text| text == "hold this turn")
    );
    assert!(!queue_has_prompt(
        &reservation.steer_input_queue,
        "hold this turn"
    ));
    assert!(!queue_has_prompt(
        &reservation.pending_turn_queue,
        "hold this turn"
    ));
    assert!(queue_has_prompt(
        &reservation.steer_input_queue,
        "scheduled steer"
    ));
    assert!(!queue_has_prompt(
        &reservation.pending_turn_queue,
        "scheduled steer"
    ));
    assert!(queue_has_prompt(
        &reservation.pending_turn_queue,
        "scheduled follow-up"
    ));
    assert!(!queue_has_prompt(
        &reservation.steer_input_queue,
        "scheduled follow-up"
    ));

    open.store(true, Ordering::SeqCst);
    // Workspace tests run concurrently; allow queued turns to drain under CI load.
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let drained = runtime
                .session_turn_reservation_snapshot(session_id)
                .await
                .is_none_or(|reservation| {
                    reservation.active_turn.is_none()
                        && reservation
                            .pending_turn_queue
                            .lock()
                            .expect("pending turn queue mutex should not be poisoned")
                            .is_empty()
                        && reservation
                            .steer_input_queue
                            .lock()
                            .expect("steer input queue mutex should not be poisoned")
                            .is_empty()
                });
            if drained {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("scheduled turns did not drain after releasing the provider")?;

    assert_eq!(
        runtime
            .schedule_store
            .list(Some(&session_id), None)?
            .iter()
            .map(|job| job.run_count)
            .collect::<Vec<_>>(),
        vec![1, 1, 1, 1]
    );
    for job_id in initial_job_ids {
        runtime.schedule_store.delete(&job_id)?;
    }

    open.store(false, Ordering::SeqCst);
    let repeat_job = runtime.schedule_store.upsert(
        devo_protocol::native::rpc_schedule::SessionScheduleUpsertParams {
            kind: devo_protocol::native::rpc_schedule::ScheduleJobKind::Heartbeat,
            session_id,
            job_id: None,
            cwd: None,
            schedule: Some("every 1s".into()),
            interval_ms: None,
            prompt: None,
            instruction: Some("hold this turn".into()),
            delivery_mode: devo_protocol::native::rpc_schedule::ScheduleDeliveryMode::Steer,
            label: Some("same-text-after-turn-completes".into()),
        },
    )?;
    tokio::time::sleep(Duration::from_millis(1_050)).await;
    runtime.dispatch_due_schedule_jobs().await?;
    let repeated_turn = runtime
        .session_turn_reservation_snapshot(session_id)
        .await
        .context("repeated prompt should start a new turn after completion")?;
    assert!(repeated_turn.active_turn.is_some());
    assert!(
        runtime
            .active_turn_user_texts(
                session_id,
                repeated_turn
                    .active_turn
                    .as_ref()
                    .expect("repeated prompt turn")
                    .turn_id(),
            )
            .await
            .iter()
            .any(|text| text == "hold this turn")
    );
    assert_eq!(
        runtime
            .schedule_store
            .list(Some(&session_id), None)?
            .into_iter()
            .find(|job| job.job_id == repeat_job.job_id)
            .expect("repeated prompt schedule remains active")
            .run_count,
        1
    );
    runtime.schedule_store.delete(&repeat_job.job_id)?;
    open.store(true, Ordering::SeqCst);
    Ok(())
}

#[tokio::test]
async fn schedule_outbox_recovers_dequeue_gap_after_restart_without_duplicate() -> Result<()> {
    let open = Arc::new(AtomicBool::new(false));
    let started = Arc::new(AtomicBool::new(false));
    let data_root = TempDir::new()?;
    let runtime = TestRuntime::new(Arc::new(GatedProvider {
        open: Arc::clone(&open),
        started: Arc::clone(&started),
    }))
    .db_file("schedule-outbox-restart.db")
    .runtime(data_root.path());
    let (outbound, _receiver) = test_outbound_channel(16);
    let connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, outbound)
        .await;
    runtime
        .handle_acp_initialize(
            connection_id,
            Some(serde_json::json!(1)),
            serde_json::json!({
                "protocolVersion": 1,
                "clientCapabilities": { "terminal": false },
                "_meta": { "devo": { "protocol": "native" } },
            }),
        )
        .await;
    let session_key = Uuid::new_v4();
    let turn_key = Uuid::new_v4();
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 2,
                "method": "session/new",
                "params": {
                    "cwd": data_root.path(),
                    "idempotencyKey": format!("schedule-outbox-session-{session_key}"),
                },
            }),
        )
        .await
        .context("session/new response")?;
    let session: SuccessResponse<devo_protocol::native::rpc_session::SessionNewResult> =
        serde_json::from_value(response)?;
    let session_id = session.result.session.id;
    runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 3,
                "method": "turn/start",
                "params": {
                    "sessionId": session_id,
                    "input": [{ "type": "text", "text": "hold before restart" }],
                    "idempotencyKey": format!("schedule-outbox-turn-{turn_key}"),
                },
            }),
        )
        .await
        .context("turn/start response")?;
    wait_for_provider_start(&started).await?;

    let job = runtime.schedule_store.upsert(
        devo_protocol::native::rpc_schedule::SessionScheduleUpsertParams {
            kind: devo_protocol::native::rpc_schedule::ScheduleJobKind::Heartbeat,
            session_id,
            job_id: None,
            cwd: None,
            schedule: Some("every 1s".into()),
            interval_ms: None,
            prompt: None,
            instruction: Some("restart-safe scheduled input".into()),
            delivery_mode: devo_protocol::native::rpc_schedule::ScheduleDeliveryMode::FollowUp,
            label: Some("restart-safe-outbox".into()),
        },
    )?;
    tokio::time::sleep(Duration::from_millis(1_050)).await;
    runtime.dispatch_due_schedule_jobs().await?;
    let occurrence = runtime
        .schedule_store
        .pending_occurrences(chrono::Utc::now())?
        .into_iter()
        .find(|occurrence| occurrence.job.job_id == job.job_id)
        .context("schedule occurrence should remain until transcript materialization")?;
    let client_user_message_id = occurrence.occurrence_id;
    assert_eq!(
        scheduled_message_count(&runtime, session_id, &client_user_message_id).await?,
        1
    );
    let queued_item = runtime
        .deps
        .db
        .list_pending(&session_id, crate::db::QueueType::Turn)?
        .into_iter()
        .find(|item| {
            item.metadata.as_ref().is_some_and(|metadata| {
                metadata
                    .get("clientUserMessageId")
                    .and_then(serde_json::Value::as_str)
                    == Some(client_user_message_id.as_str())
            })
        })
        .context("scheduled follow-up should be durably queued")?;
    let notice = crate::runtime::kernel_host::dispatch_host_request(
        session_id.as_str(),
        serde_json::json!({
            "type": "bash.completed",
            "params": { "pid": 9182, "command": "sleep 60", "exitCode": 0 },
        }),
    )
    .await;
    assert_eq!(notice["result"]["accepted"], true);

    runtime.shutdown().await;
    assert!(
        crate::runtime::kernel_host::take_bash_notices(session_id.as_str()).is_empty(),
        "kernel teardown must discard notices tied to its in-memory Bash handles"
    );
    assert!(runtime.schedule_wake_cancel.is_cancelled());
    assert!(runtime.deps.db.remove_pending_by_id(
        &session_id,
        crate::db::QueueType::Turn,
        &queued_item.id,
    )?);
    assert_eq!(
        scheduled_message_count(&runtime, session_id, &client_user_message_id).await?,
        0,
        "sabotage removes the queue row before Native transcript materialization"
    );
    drop(runtime);

    let restarted = TestRuntime::new(Arc::new(GatedProvider {
        open: Arc::clone(&open),
        started: Arc::new(AtomicBool::new(false)),
    }))
    .db_file("schedule-outbox-restart.db")
    .runtime(data_root.path());
    restarted.dispatch_due_schedule_jobs().await?;
    assert_eq!(
        scheduled_message_count(&restarted, session_id, &client_user_message_id).await?,
        1,
        "durable outbox must re-admit the occurrence after the queue row is lost"
    );
    restarted.dispatch_due_schedule_jobs().await?;
    assert_eq!(
        scheduled_message_count(&restarted, session_id, &client_user_message_id).await?,
        1,
        "repeated wake must not duplicate an accepted occurrence"
    );

    open.store(true, Ordering::SeqCst);
    restarted.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn inactive_schedules_discard_claimed_but_undelivered_occurrences() -> Result<()> {
    let open = Arc::new(AtomicBool::new(false));
    let started = Arc::new(AtomicBool::new(false));
    let data_root = TempDir::new()?;
    let runtime = TestRuntime::new(Arc::new(GatedProvider {
        open: Arc::clone(&open),
        started: Arc::clone(&started),
    }))
    .db_file("schedule-inactive-outbox.db")
    .runtime(data_root.path());
    let (outbound, _receiver) = test_outbound_channel(16);
    let connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, outbound)
        .await;
    runtime
        .handle_acp_initialize(
            connection_id,
            Some(serde_json::json!(1)),
            serde_json::json!({
                "protocolVersion": 1,
                "clientCapabilities": { "terminal": false },
                "_meta": { "devo": { "protocol": "native" } },
            }),
        )
        .await;
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 2,
                "method": "session/new",
                "params": {
                    "cwd": data_root.path(),
                    "idempotencyKey": format!("inactive-schedule-session-{}", Uuid::new_v4()),
                },
            }),
        )
        .await
        .context("session/new response")?;
    let session: SuccessResponse<devo_protocol::native::rpc_session::SessionNewResult> =
        serde_json::from_value(response)?;
    let session_id = session.result.session.id;

    let mut jobs = Vec::new();
    for label in [
        "paused-before-dispatch",
        "stopped-before-dispatch",
        "deleted-before-dispatch",
    ] {
        jobs.push(runtime.schedule_store.upsert(
            devo_protocol::native::rpc_schedule::SessionScheduleUpsertParams {
                kind: devo_protocol::native::rpc_schedule::ScheduleJobKind::Heartbeat,
                session_id,
                job_id: None,
                cwd: None,
                schedule: Some("every 1h".into()),
                interval_ms: None,
                prompt: None,
                instruction: Some(label.into()),
                delivery_mode: devo_protocol::native::rpc_schedule::ScheduleDeliveryMode::FollowUp,
                label: Some(label.into()),
            },
        )?);
    }
    // Keep the real scheduler from racing this test. The test advances its
    // manual claim time past the fixture interval, then gates dispatch.
    let due_at = chrono::Utc::now() + chrono::Duration::hours(2);
    runtime.schedule_store.claim_due_with_outbox(due_at)?;
    assert_eq!(
        runtime
            .schedule_store
            .pending_occurrences(chrono::Utc::now())?
            .len(),
        3,
        "all jobs are durably claimed before lifecycle changes"
    );

    let dispatch_guard = runtime.schedule_store.lock_dispatch().await;
    let stop_runtime = Arc::clone(&runtime);
    let stop_job_id = jobs[1].job_id;
    let stop_request = tokio::spawn(async move {
        stop_runtime
            .handle_native_session_schedule_update(
                serde_json::json!(11),
                serde_json::json!({ "jobId": stop_job_id, "action": "stop" }),
            )
            .await
    });
    tokio::task::yield_now().await;
    assert!(
        !stop_request.is_finished(),
        "a schedule stop must serialize behind an in-progress dispatch"
    );
    // Keep the dispatcher gated while the lifecycle changes win. Public
    // handlers queue behind this lock and are exercised separately; updating
    // the store here makes the claimed-but-undelivered state deterministic.
    runtime.schedule_store.update(
        &jobs[0].job_id,
        devo_protocol::native::rpc_schedule::ScheduleUpdateAction::Pause,
    )?;
    runtime.schedule_store.update(
        &jobs[1].job_id,
        devo_protocol::native::rpc_schedule::ScheduleUpdateAction::Stop,
    )?;
    runtime.schedule_store.delete(&jobs[2].job_id)?;
    drop(dispatch_guard);
    let stop_response = stop_request.await.context("stop response")?;
    assert!(
        stop_response.get("result").is_some(),
        "schedule stop should succeed"
    );

    runtime.dispatch_due_schedule_jobs().await?;
    for occurrence in runtime
        .schedule_store
        .pending_occurrences(chrono::Utc::now())?
    {
        assert!(
            jobs.iter().all(|job| job.job_id != occurrence.job.job_id),
            "inactive schedule occurrences must be retired"
        );
    }
    assert!(
        runtime
            .schedule_store
            .pending_occurrences(chrono::Utc::now())?
            .is_empty(),
        "inactive occurrences must not remain retryable"
    );
    for job in jobs {
        let occurrence_id = format!(
            "schedule:{}:{}",
            job.job_id.as_str(),
            job.next_run_at.expect("schedule time").timestamp_micros()
        );
        let message_count = scheduled_message_count(&runtime, session_id, &occurrence_id).await?;
        assert_eq!(
            message_count,
            0,
            "inactive occurrence {} must not reach the queue or transcript; current job: {:?}",
            job.job_id,
            runtime.schedule_store.get_job(&job.job_id)?
        );
    }

    open.store(true, Ordering::SeqCst);
    runtime.shutdown().await;
    Ok(())
}

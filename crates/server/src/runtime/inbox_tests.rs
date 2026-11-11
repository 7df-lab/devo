use super::*;
use anyhow::{Context, Result};
use async_trait::async_trait;
use devo_protocol::{
    ModelRequest, ModelResponse, ResponseContent, ResponseMetadata, StopReason, StreamEvent, Usage,
};
use devo_provider::ModelProviderSDK;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::{pin::Pin, sync::Mutex as StdMutex, time::Duration};
use tokio::sync::Semaphore;

struct InboxProvider {
    requests: StdMutex<Vec<ModelRequest>>,
    started: Semaphore,
    release: Semaphore,
}
#[async_trait]
impl ModelProviderSDK for InboxProvider {
    fn name(&self) -> &str {
        "inbox-fifo"
    }
    async fn completion(&self, _: ModelRequest) -> Result<ModelResponse> {
        anyhow::bail!("unexpected auxiliary call")
    }
    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        self.requests.lock().unwrap().push(request);
        self.started.add_permits(1);
        self.release.acquire().await?.forget();
        Ok(Box::pin(futures::stream::iter([Ok(
            StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "fifo-reply".into(),
                    content: vec![ResponseContent::Text("done".into())],
                    stop_reason: Some(StopReason::EndTurn),
                    usage: Usage::default(),
                    metadata: ResponseMetadata::default(),
                },
            },
        )])))
    }
}

#[tokio::test]
async fn followup_head_blocks_later_steer_until_next_turn() -> Result<()> {
    let home = tempfile::tempdir()?;
    let provider = Arc::new(InboxProvider {
        requests: StdMutex::new(Vec::new()),
        started: Semaphore::new(/*permits*/ 0),
        release: Semaphore::new(/*permits*/ 0),
    });
    let runtime = crate::test_support::TestRuntime::new(provider.clone())
        .with_test_model()
        .runtime(home.path());
    let (tx, mut rx) = crate::test_outbound_channel(512);
    let connection = runtime
        .register_connection(ClientTransportKind::Stdio, tx)
        .await;
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    runtime.handle_incoming(connection,json!({"id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{},"_meta":{"devo":{"protocol":"native"}},"clientInfo":{"name":"fifo","title":"test","version":"1"}}})).await;
    let created = runtime.handle_incoming(connection,json!({"id":2,"method":"session/new","params":{"cwd":home.path(),"idempotencyKey":"fifo-session"}})).await.context("new session")?;
    let session = SessionId::from(
        created["result"]["session"]["id"]
            .as_str()
            .context("session id")?,
    );
    let started = runtime.handle_incoming(connection,json!({"id":3,"method":"turn/start","params":{"sessionId":session,"input":[{"type":"text","text":"INITIAL"}],"idempotencyKey":"initial"}})).await.context("start")?;
    assert!(started.get("error").is_none(), "{started}");
    tokio::time::timeout(Duration::from_secs(/*secs*/ 15), provider.started.acquire())
        .await??
        .forget();
    let turn_id = started["result"]["turn"]["id"].clone();
    let a = runtime.handle_incoming(connection,json!({"id":4,"method":"session/queue/push","params":{"sessionId":session,"input":[{"type":"text","text":"FOLLOW_A"}],"idempotencyKey":"a"}})).await.context("queue A")?;
    assert!(a.get("error").is_none(), "{a}");
    let b = runtime.handle_incoming(connection,json!({"id":5,"method":"turn/steer","params":{"sessionId":session,"expectedTurnId":turn_id,"input":[{"type":"text","text":"STEER_B"}],"idempotencyKey":"b"}})).await.context("queue B")?;
    assert!(b.get("error").is_none(), "{b}");
    let queued = runtime
        .deps
        .db
        .list_pending(&session, crate::db::QueueType::Turn)?;
    assert_eq!(
        queued
            .iter()
            .map(|item| item.delivery())
            .collect::<Vec<_>>(),
        vec![
            devo_protocol::native::host_message::HostMessageDelivery::FollowUp,
            devo_protocol::native::host_message::HostMessageDelivery::Steer
        ]
    );
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    provider.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(/*secs*/ 15), provider.started.acquire())
        .await??
        .forget();
    let second = serde_json::to_string(&provider.requests.lock().unwrap()[1])?;
    assert!(
        second.find("FOLLOW_A").context("A in model")?
            < second.find("STEER_B").context("B in model")?
    );
    let active = runtime
        .runtime_active_turn_id(session)
        .await
        .context("active second turn")?;
    assert_ne!(json!(active), turn_id);
    let retry = runtime.handle_incoming(connection,json!({"id":6,"method":"session/queue/push","params":{"sessionId":session,"input":[{"type":"text","text":"FOLLOW_A"}],"idempotencyKey":"a"}})).await.context("retry A")?;
    assert_eq!(retry["result"]["outcome"], json!("alreadyAccepted"));
    assert!(
        runtime
            .deps
            .db
            .list_pending(&session, crate::db::QueueType::Turn)?
            .is_empty()
    );
    // A steer at the head is consumed on the next model boundary of the SAME
    // turn, without requiring a keyboard event or another client request.
    let steer = runtime.handle_incoming(connection,json!({"id":7,"method":"turn/steer","params":{"sessionId":session,"expectedTurnId":active,"input":[{"type":"text","text":"STEER_C"}],"idempotencyKey":"c"}})).await.context("head steer")?;
    assert!(steer.get("error").is_none(), "{steer}");
    provider.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(/*secs*/ 15), provider.started.acquire())
        .await??
        .forget();
    assert_eq!(runtime.runtime_active_turn_id(session).await, Some(active));
    let third = serde_json::to_string(&provider.requests.lock().unwrap()[2])?;
    assert!(
        third.find("STEER_B").context("B in request")?
            < third.find("STEER_C").context("C in request")?
    );
    provider.release.add_permits(10);
    runtime.shutdown().await;
    drain.abort();
    Ok(())
}

#[tokio::test]
async fn failed_turn_start_persistence_restores_claimed_fifo_barrier() -> Result<()> {
    let home = tempfile::tempdir()?;
    let provider = Arc::new(InboxProvider {
        requests: StdMutex::new(Vec::new()),
        started: Semaphore::new(/*permits*/ 0),
        release: Semaphore::new(/*permits*/ 0),
    });
    let runtime = crate::test_support::TestRuntime::new(provider.clone())
        .with_test_model()
        .runtime(home.path());
    let (tx, mut rx) = crate::test_outbound_channel(512);
    let connection = runtime
        .register_connection(ClientTransportKind::Stdio, tx)
        .await;
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    runtime.handle_incoming(connection,json!({"id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{},"_meta":{"devo":{"protocol":"native"}},"clientInfo":{"name":"failure-test","title":"test","version":"1"}}})).await;
    let created = runtime.handle_incoming(connection,json!({"id":2,"method":"session/new","params":{"cwd":home.path(),"idempotencyKey":"failure-session"}})).await.context("new session")?;
    let session = SessionId::from(
        created["result"]["session"]["id"]
            .as_str()
            .context("session id")?,
    );
    let handle = runtime.session(session).await.context("handle")?;
    let path = handle
        .rollout_path()
        .await
        .flatten()
        .context("rollout path")?;
    // Close the temporary journal while keeping the actor alive, injecting a
    // deterministic append failure after the inbox has been claimed.
    runtime
        .rollout_store
        .delete_session_journal(&session, Some(&path), None, true)?;
    let started = runtime.handle_incoming(connection,json!({"id":3,"method":"turn/start","params":{"sessionId":session,"input":[{"type":"text","text":"must remain visible"}],"idempotencyKey":"failure-input"}})).await.context("failed start")?;
    assert!(started.get("error").is_some(), "{started}");
    let listed = runtime
        .handle_incoming(
            connection,
            json!({"id":4,"method":"session/queue/list","params":{"sessionId":session}}),
        )
        .await
        .context("queue list")?;
    assert_eq!(
        listed["result"]["entries"].as_array().map(Vec::len),
        Some(1)
    );
    assert_eq!(
        listed["result"]["entries"][0]["recoveryRequired"],
        json!(true)
    );
    let later = runtime.handle_incoming(connection,json!({"id":5,"method":"turn/start","params":{"sessionId":session,"input":[{"type":"text","text":"cannot overtake"}],"idempotencyKey":"later"}})).await.context("later start")?;
    assert!(later.get("error").is_some(), "{later}");
    assert!(provider.requests.lock().unwrap().is_empty());
    runtime.shutdown().await;
    drain.abort();
    Ok(())
}

#[tokio::test]
async fn failed_steer_history_persistence_holds_unconsumed_prefix() -> Result<()> {
    let home = tempfile::tempdir()?;
    let provider = Arc::new(InboxProvider {
        requests: StdMutex::new(Vec::new()),
        started: Semaphore::new(/*permits*/ 0),
        release: Semaphore::new(/*permits*/ 0),
    });
    let runtime = crate::test_support::TestRuntime::new(provider.clone())
        .with_test_model()
        .runtime(home.path());
    let (tx, mut rx) = crate::test_outbound_channel(512);
    let connection = runtime
        .register_connection(ClientTransportKind::Stdio, tx)
        .await;
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    runtime.handle_incoming(connection,json!({"id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{},"_meta":{"devo":{"protocol":"native"}},"clientInfo":{"name":"steer-failure","title":"test","version":"1"}}})).await;
    let created = runtime.handle_incoming(connection,json!({"id":2,"method":"session/new","params":{"cwd":home.path(),"idempotencyKey":"steer-failure-session"}})).await.context("new")?;
    let session = SessionId::from(created["result"]["session"]["id"].as_str().context("id")?);
    let started = runtime.handle_incoming(connection,json!({"id":3,"method":"turn/start","params":{"sessionId":session,"input":[{"type":"text","text":"INITIAL"}],"idempotencyKey":"initial"}})).await.context("start")?;
    tokio::time::timeout(Duration::from_secs(/*secs*/ 15), provider.started.acquire())
        .await??
        .forget();
    let turn_id = started["result"]["turn"]["id"].clone();
    for index in 0..2 {
        let admitted = runtime.handle_incoming(connection,json!({"id":4+index,"method":"turn/steer","params":{"sessionId":session,"expectedTurnId":turn_id,"input":[{"type":"text","text":format!("STEER_{index}")}],"idempotencyKey":format!("steer-{index}")}})).await.context("steer")?;
        assert!(admitted.get("error").is_none(), "{admitted}");
    }
    let handle = runtime.session(session).await.context("handle")?;
    let path = handle.rollout_path().await.flatten().context("rollout")?;
    runtime
        .rollout_store
        .delete_session_journal(&session, Some(&path), None, true)?;
    // Invoke the same host boundary while the provider is gated so the
    // injected journal failure targets steering persistence, not response I/O.
    runtime
        .claim_inbox_steers(session, TurnId::from(turn_id.as_str().unwrap()))
        .await
        .expect_err("steer history append must fail");
    provider.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(/*secs*/ 15), async {
        loop {
            let snapshot = handle.turn_reservation_snapshot().await.unwrap();
            if runtime.runtime_active_turn_id(session).await.is_none()
                && snapshot.pending_turn_queue.lock().unwrap().len() == 2
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(/*millis*/ 10)).await;
        }
    })
    .await
    .context("failed boundary did not settle")?;
    let listed = runtime
        .handle_incoming(
            connection,
            json!({"id":8,"method":"session/queue/list","params":{"sessionId":session}}),
        )
        .await
        .context("list")?;
    let entries = listed["result"]["entries"].as_array().context("entries")?;
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["recoveryRequired"], json!(true), "{listed}");
    assert_eq!(entries[0]["preview"], "STEER_0");
    assert_eq!(entries[1]["preview"], "STEER_1");
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    runtime.shutdown().await;
    drain.abort();
    Ok(())
}

#[tokio::test]
async fn interrupt_between_host_admission_and_drain_keeps_result_held() -> Result<()> {
    let home = tempfile::tempdir()?;
    let provider = Arc::new(InboxProvider {
        requests: StdMutex::new(Vec::new()),
        started: Semaphore::new(/*permits*/ 0),
        release: Semaphore::new(/*permits*/ 0),
    });
    let runtime = crate::test_support::TestRuntime::new(provider.clone())
        .with_test_model()
        .runtime(home.path());
    let (tx, mut rx) = crate::test_outbound_channel(512);
    let connection = runtime
        .register_connection(ClientTransportKind::Stdio, tx)
        .await;
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    runtime.handle_incoming(connection,json!({"id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{},"_meta":{"devo":{"protocol":"native"}},"clientInfo":{"name":"interrupt-race","title":"test","version":"1"}}})).await;
    let created = runtime.handle_incoming(connection,json!({"id":2,"method":"session/new","params":{"cwd":home.path(),"idempotencyKey":"interrupt-session"}})).await.context("new")?;
    let session = SessionId::from(created["result"]["session"]["id"].as_str().context("id")?);
    let handle = runtime.session(session).await.context("handle")?;
    let gate = handle.lock_state_change().await;
    let snapshot = runtime
        .session_turn_reservation_snapshot(session)
        .await
        .context("snapshot")?;
    let item = devo_protocol::PendingInputItem::new(
        devo_protocol::PendingInputKind::UserText {
            text: "completed work".into(),
        },
        Some(
            json!({"host_message_origin":{"eventId":"race-completion","source":"agent","sender":"child","delivery":"followUp"}}),
        ),
        chrono::Utc::now(),
    );
    runtime.persist_inbox_item(
        session,
        snapshot.ephemeral,
        &snapshot.pending_turn_queue,
        &item,
    )?;
    // Precisely model the interrupt winning after commit but before the host's
    // drain obtains this same gate; no scheduling sleeps are involved.
    runtime.suspend_background_input(session);
    drop(gate);
    runtime.drain_queue_if_idle(session).await;
    let snapshot = handle
        .turn_reservation_snapshot()
        .await
        .context("held snapshot")?;
    let pending = snapshot
        .pending_turn_queue
        .lock()
        .unwrap()
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, item.id);
    assert_eq!(
        pending[0].metadata.as_ref().unwrap()["ownerInterrupted"],
        true
    );
    assert!(crate::runtime::handlers::queue_steer::requires_queue_review(&pending[0]));
    assert!(provider.requests.lock().unwrap().is_empty());
    let retry = runtime.handle_incoming(connection, json!({"id":3,"method":"session/queue/retry","params":{"sessionId":session,"queueItemId":item.id}})).await.context("retry")?;
    assert!(retry.get("error").is_none(), "{retry}");
    tokio::time::timeout(Duration::from_secs(/*secs*/ 15), provider.started.acquire())
        .await??
        .forget();
    assert!(!runtime.background_input_suspended(session));
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    provider.release.add_permits(1);
    runtime.shutdown().await;
    drain.abort();
    Ok(())
}

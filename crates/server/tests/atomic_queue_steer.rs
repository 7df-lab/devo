//! Native RPC coverage for identity-based transfers, races, and held recovery.
use anyhow::{Context, Result};
use async_trait::async_trait;
use devo_protocol::native::ids::{SessionId, TurnId};
use devo_protocol::{
    ModelRequest, ModelResponse, PendingInputItem, PendingInputKind, ResponseContent,
    ResponseMetadata, StopReason, StreamEvent, Usage,
};
use devo_provider::ModelProviderSDK;
use devo_server::{
    ClientTransportKind, ServerRuntime,
    db::{Database, QueueType},
    test_support::TestRuntime,
};
use futures::{Stream, StreamExt};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::{
    path::Path,
    pin::Pin,
    sync::{Arc, Mutex},
};
use tempfile::TempDir;
use tokio::{
    sync::{Notify, mpsc},
    time::{Duration, timeout},
};

#[derive(Default)]
enum ProviderBehavior {
    #[default]
    Succeed,
    FailThenRecover,
}

#[derive(Default)]
struct GatedProvider {
    behavior: ProviderBehavior,
    requests: Mutex<Vec<Value>>,
    started: Notify,
    release: Arc<Notify>,
}
fn response() -> ModelResponse {
    ModelResponse {
        id: "response".into(),
        content: vec![ResponseContent::Text("Done".into())],
        stop_reason: Some(StopReason::EndTurn),
        usage: Usage::default(),
        metadata: ResponseMetadata::default(),
    }
}
#[async_trait]
impl ModelProviderSDK for GatedProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(response())
    }
    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let request_number = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(serde_json::to_value(request)?);
            requests.len()
        };
        let fail_then_recover = matches!(self.behavior, ProviderBehavior::FailThenRecover);
        let release = Arc::clone(&self.release);
        self.started.notify_one();
        Ok(futures::stream::once(async move {
            if request_number == 1 || (fail_then_recover && request_number == 2) {
                release.notified().await;
            }
            if fail_then_recover && request_number == 1 {
                return Err(devo_provider::error::ProviderError::InvalidRequestError {
                    message: "synthetic failure before explicit recovery".into(),
                    details: None,
                }
                .into());
            }
            Ok(StreamEvent::MessageDone {
                response: response(),
            })
        })
        .boxed())
    }
    fn name(&self) -> &str {
        "gated-queue-provider"
    }
}
fn runtime(root: &Path, db: Arc<Database>, provider: Arc<GatedProvider>) -> Arc<ServerRuntime> {
    TestRuntime::new(provider)
        .disabled_skills()
        .database(db)
        .runtime(root)
}
async fn rpc(runtime: &Arc<ServerRuntime>, connection: u64, method: &str, params: Value) -> Value {
    runtime
        .handle_incoming(
            connection,
            json!({"id":uuid::Uuid::new_v4().to_string(),"method":method,"params":params}),
        )
        .await
        .expect("RPC response")
}
async fn connect(runtime: &Arc<ServerRuntime>) -> (u64, mpsc::Receiver<Value>) {
    let (tx, rx) = devo_server::test_outbound_channel(4096);
    let connection = runtime
        .register_connection(ClientTransportKind::Stdio, tx)
        .await;
    let reply = rpc(runtime, connection, "initialize", json!({"protocolVersion":1,"clientCapabilities":{},
        "_meta":{"devo":{"protocol":"native"}},"clientInfo":{"name":"queue-test","title":"Queue test","version":"1"}})).await;
    assert!(reply.get("error").is_none(), "{reply}");
    (connection, rx)
}
async fn subscribe(runtime: &Arc<ServerRuntime>, connection: u64, session: SessionId) {
    let reply = rpc(
        runtime,
        connection,
        "subscription/create",
        json!({"selectors":[{"kind":"session","sessionId":session}],"includeSnapshot":false}),
    )
    .await;
    assert!(reply.get("error").is_none(), "{reply}");
}
async fn completed(rx: &mut mpsc::Receiver<Value>, count: usize) -> Result<()> {
    timeout(Duration::from_secs(20), async {
        let mut seen = 0;
        while let Some(value) = rx.recv().await {
            if value["method"] == "turn/completed" {
                seen += 1;
                if seen == count {
                    return Ok(());
                }
            }
        }
        anyhow::bail!("closed before terminal notifications")
    })
    .await
    .context("terminal notification timeout")?
}
struct Fixture {
    temp: TempDir,
    db: Arc<Database>,
    provider: Arc<GatedProvider>,
    runtime: Arc<ServerRuntime>,
    connection: u64,
    notifications: mpsc::Receiver<Value>,
    session: SessionId,
    turn: TurnId,
}
impl Fixture {
    async fn new() -> Result<Self> {
        Self::with_provider(Arc::new(GatedProvider::default())).await
    }

    async fn with_provider(provider: Arc<GatedProvider>) -> Result<Self> {
        let temp = TempDir::new()?;
        let db = Arc::new(Database::open(temp.path().join("atomic_queue.db"))?);
        let runtime = runtime(temp.path(), Arc::clone(&db), Arc::clone(&provider));
        let (connection, notifications) = connect(&runtime).await;
        let created = rpc(
            &runtime,
            connection,
            "session/new",
            json!({"cwd":temp.path(),"idempotencyKey":"new"}),
        )
        .await;
        let session = SessionId::from(
            created["result"]["session"]["id"]
                .as_str()
                .expect("session id"),
        );
        subscribe(&runtime, connection, session).await;
        let started = rpc(
            &runtime,
            connection,
            "turn/start",
            json!({
                "sessionId": session,
                "input": [{"type": "text", "text": "initial"}],
                "idempotencyKey": "start"
            }),
        )
        .await;
        let turn = TurnId::from(started["result"]["turn"]["id"].as_str().expect("turn id"));
        timeout(Duration::from_secs(10), provider.started.notified()).await?;
        Ok(Self {
            temp,
            db,
            provider,
            runtime,
            connection,
            notifications,
            session,
            turn,
        })
    }
    async fn queue(&self, text: &str) -> Value {
        let reply = rpc(
            &self.runtime,
            self.connection,
            "session/queue/push",
            json!({
                "sessionId": self.session,
                "input": [
                    {"type": "text", "text": text},
                    {"type": "text", "text": "second preserved part"}
                ],
                "idempotencyKey": uuid::Uuid::new_v4().to_string()
            }),
        )
        .await;
        assert_eq!(reply["result"]["outcome"], "queued");
        let list = rpc(
            &self.runtime,
            self.connection,
            "session/queue/list",
            json!({"sessionId": self.session}),
        )
        .await;
        list["result"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["queueItemId"] == reply["result"]["entry"]["queueItemId"])
            .unwrap()
            .clone()
    }
    fn params(&self, entry: &Value) -> Value {
        json!({"sessionId":self.session,"queueItemId":entry["queueItemId"],"expectedTurnId":self.turn})
    }
    async fn steer(&self, entry: &Value) -> Value {
        rpc(
            &self.runtime,
            self.connection,
            "session/queue/steer",
            self.params(entry),
        )
        .await
    }
}

#[tokio::test]
async fn concurrent_retries_return_one_receipt_and_transfer_complete_original() -> Result<()> {
    let mut f = Fixture::new().await?;
    let entry = f.queue("original").await;
    let (a, b) = tokio::join!(f.steer(&entry), f.steer(&entry));
    let expected = json!({"outcome":"accepted","queueItemId":entry["queueItemId"],"turnId":f.turn});
    assert_eq!(
        (a["result"].clone(), b["result"].clone()),
        (expected.clone(), expected.clone())
    );
    let pending = f.db.list_pending(&f.session, QueueType::Steer)?;
    assert_eq!(pending.len(), 1);
    let PendingInputKind::UserInput { input, .. } = &pending[0].kind else {
        panic!("full input required")
    };
    assert_eq!(serde_json::to_value(input)?, entry["input"]);
    assert!(f.db.list_pending(&f.session, QueueType::Turn)?.is_empty());
    // Lost response, empty queue cache, and unknown current turn still recover.
    let retried = rpc(
        &f.runtime,
        f.connection,
        "session/queue/steer",
        json!({"sessionId":f.session,"queueItemId":entry["queueItemId"],"expectedTurnId":""}),
    )
    .await;
    assert_eq!(retried["result"], expected);
    f.provider.release.notify_one();
    completed(&mut f.notifications, 1).await?;
    assert_eq!(f.provider.requests.lock().unwrap().len(), 2);
    Ok(())
}
#[tokio::test]
async fn stale_turn_preserves_input_and_other_sessions_cannot_claim_it() -> Result<()> {
    let mut f = Fixture::new().await?;
    let entry = f.queue("preserved").await;
    let mut params = f.params(&entry);
    params["expectedTurnId"] = json!("stale");
    let reply = rpc(&f.runtime, f.connection, "session/queue/steer", params).await;
    assert_eq!(
        reply["result"],
        json!({"outcome":"keptQueued","entry":entry})
    );
    let created = rpc(
        &f.runtime,
        f.connection,
        "session/new",
        json!({"cwd":f.temp.path(),"idempotencyKey":"other"}),
    )
    .await;
    let mut params = f.params(&entry);
    params["sessionId"] = created["result"]["session"]["id"].clone();
    assert!(
        rpc(&f.runtime, f.connection, "session/queue/steer", params)
            .await
            .get("error")
            .is_some()
    );
    assert!(f.db.list_pending(&f.session, QueueType::Steer)?.is_empty());
    f.provider.release.notify_one();
    completed(&mut f.notifications, 2).await?;
    Ok(())
}
#[tokio::test]
async fn concurrent_update_and_transfer_choose_one_full_payload() -> Result<()> {
    let mut f = Fixture::new().await?;
    let entry = f.queue("old").await;
    let replacement =
        json!([{"type":"text","text":"new"},{"type":"text","text":"new second part"}]);
    let (changed, steered) = tokio::join!(
        rpc(
            &f.runtime,
            f.connection,
            "session/queue/update",
            json!({"sessionId":f.session,"queueItemId":entry["queueItemId"],"input":replacement})
        ),
        f.steer(&entry)
    );
    assert_eq!(steered["result"]["outcome"], "accepted");
    let pending = f.db.list_pending(&f.session, QueueType::Steer)?;
    let PendingInputKind::UserInput { input, .. } = &pending[0].kind else {
        panic!("full input required")
    };
    assert_eq!(
        serde_json::to_value(input)?,
        if changed.get("error").is_none() {
            replacement
        } else {
            entry["input"].clone()
        }
    );
    f.provider.release.notify_one();
    completed(&mut f.notifications, 1).await?;
    Ok(())
}
#[tokio::test]
async fn concurrent_remove_and_transfer_have_one_owner() -> Result<()> {
    let mut f = Fixture::new().await?;
    let entry = f.queue("race").await;
    let (removed, steered) = tokio::join!(
        rpc(
            &f.runtime,
            f.connection,
            "session/queue/remove",
            json!({"sessionId":f.session,"queueItemId":entry["queueItemId"]})
        ),
        f.steer(&entry)
    );
    assert_eq!(
        (removed.get("error").is_none() as u8) + (steered.get("error").is_none() as u8),
        1
    );
    let accepted = steered.get("error").is_none();
    assert_eq!(
        f.db.list_pending(&f.session, QueueType::Steer)?.len(),
        usize::from(accepted)
    );
    f.provider.release.notify_one();
    completed(&mut f.notifications, 1).await?;
    Ok(())
}
#[tokio::test]
async fn identical_text_with_distinct_ids_is_not_deduplicated() -> Result<()> {
    let mut f = Fixture::new().await?;
    let a = f.queue("same").await;
    let b = f.queue("same").await;
    assert_ne!(a["queueItemId"], b["queueItemId"]);
    assert_eq!(f.steer(&a).await["result"]["outcome"], "accepted");
    assert_eq!(f.steer(&b).await["result"]["outcome"], "accepted");
    assert_eq!(f.db.list_pending(&f.session, QueueType::Steer)?.len(), 2);
    f.provider.release.notify_one();
    completed(&mut f.notifications, 1).await?;
    Ok(())
}
#[tokio::test]
async fn terminal_race_never_duplicates_the_original() -> Result<()> {
    for _ in 0..8 {
        let f = Fixture::new().await?;
        let entry = f.queue("terminal race").await;
        let (reply, ()) = tokio::join!(f.steer(&entry), async {
            f.provider.release.notify_one();
        });
        if reply.get("error").is_some() {
            assert!(
                reply["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("no longer queued")
            );
        } else {
            assert!(
                ["accepted", "keptQueued"].contains(&reply["result"]["outcome"].as_str().unwrap())
            );
        }
        timeout(Duration::from_secs(20), async {
            loop {
                let read = rpc(
                    &f.runtime,
                    f.connection,
                    "session/read",
                    json!({"sessionId":f.session}),
                )
                .await;
                let queue = rpc(
                    &f.runtime,
                    f.connection,
                    "session/queue/list",
                    json!({"sessionId":f.session}),
                )
                .await;
                if f.provider.requests.lock().unwrap().len() >= 2
                    && read["result"]["session"]["status"] == "idle"
                    && queue["result"]["entries"] == json!([])
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await?;
        assert_eq!(f.provider.requests.lock().unwrap().len(), 2);
        assert!(f.db.list_pending(&f.session, QueueType::Turn)?.is_empty());
        assert!(f.db.list_pending(&f.session, QueueType::Steer)?.is_empty());
    }
    Ok(())
}

enum RecoveryQueue {
    HeldOnly,
    HeldWithFollowup,
}

async fn recovered_fixture(
    mode: RecoveryQueue,
) -> Result<(
    TempDir,
    Arc<Database>,
    Arc<GatedProvider>,
    Arc<ServerRuntime>,
    u64,
    mpsc::Receiver<Value>,
    SessionId,
    PendingInputItem,
)> {
    let temp = TempDir::new()?;
    let db = Arc::new(Database::open(temp.path().join("atomic_queue.db"))?);
    let provider = Arc::new(GatedProvider::default());
    let first = runtime(temp.path(), Arc::clone(&db), Arc::clone(&provider));
    let (connection, _rx) = connect(&first).await;
    let created = rpc(
        &first,
        connection,
        "session/new",
        json!({"cwd":temp.path(),"idempotencyKey":"new"}),
    )
    .await;
    let session = SessionId::from(created["result"]["session"]["id"].as_str().unwrap());
    let item = PendingInputItem::new(
        PendingInputKind::UserText {
            text: "uncertain prior effects".into(),
        },
        None,
        chrono::Utc::now(),
    );
    db.push_pending(&session, QueueType::Turn, &item)?;
    assert!(db.claim_queue_steer(&session, &item, &TurnId::from("old-turn"))?);
    drop(first);
    if matches!(mode, RecoveryQueue::HeldWithFollowup) {
        // A new explicit queue/push on idle starts a new turn by contract;
        // this fixture models input already behind the held FIFO head.
        db.recover_interrupted_queue_steers(&session)?;
        let next = PendingInputItem::new(
            PendingInputKind::UserText {
                text: "ordinary".into(),
            },
            None,
            chrono::Utc::now(),
        );
        db.push_pending(&session, QueueType::Turn, &next)?;
    }
    let resumed = runtime(temp.path(), Arc::clone(&db), Arc::clone(&provider));
    let (connection, notifications) = connect(&resumed).await;
    subscribe(&resumed, connection, session).await;
    let reply = rpc(
        &resumed,
        connection,
        "session/resume",
        json!({"sessionId":session}),
    )
    .await;
    assert!(reply.get("error").is_none(), "{reply}");
    Ok((
        temp,
        db,
        provider,
        resumed,
        connection,
        notifications,
        session,
        item,
    ))
}
#[tokio::test]
async fn restart_holds_input_and_explicit_retry_creates_a_fresh_identity() -> Result<()> {
    let (_temp, db, provider, runtime, connection, mut notifications, session, item) =
        recovered_fixture(RecoveryQueue::HeldOnly).await?;
    let listed = rpc(
        &runtime,
        connection,
        "session/queue/list",
        json!({"sessionId":session}),
    )
    .await;
    assert_eq!(listed["result"]["entries"][0]["recoveryRequired"], true);
    assert!(provider.requests.lock().unwrap().is_empty());
    for params in [
        json!({"sessionId":session,"queueItemId":item.id,"position":1}),
        json!({"sessionId":session,"queueItemId":item.id,"input":[{"type":"text","text":"bypass"}]}),
    ] {
        assert!(
            rpc(&runtime, connection, "session/queue/update", params)
                .await
                .get("error")
                .is_some()
        );
    }
    let receipt = rpc(
        &runtime,
        connection,
        "session/queue/steer",
        json!({"sessionId":session,"queueItemId":item.id,"expectedTurnId":"other"}),
    )
    .await;
    assert_eq!(
        receipt["result"],
        json!({"outcome":"accepted","queueItemId":item.id,"turnId":"old-turn"})
    );
    assert!(provider.requests.lock().unwrap().is_empty());
    let retried = rpc(
        &runtime,
        connection,
        "session/queue/retry",
        json!({"sessionId":session,"queueItemId":item.id}),
    )
    .await;
    assert_eq!(retried["result"], json!({}));
    timeout(Duration::from_secs(10), provider.started.notified()).await?;
    assert!(
        db.list_pending(&session, QueueType::Turn)?
            .iter()
            .all(|row| row.id != item.id)
    );
    assert!(
        rpc(
            &runtime,
            connection,
            "session/queue/retry",
            json!({"sessionId":session,"queueItemId":item.id})
        )
        .await
        .get("error")
        .is_some()
    );
    provider.release.notify_one();
    completed(&mut notifications, 1).await?;
    Ok(())
}
#[tokio::test]
async fn removing_recovered_fifo_head_releases_ordinary_input_behind_it() -> Result<()> {
    let (_temp, _db, provider, runtime, connection, mut notifications, session, item) =
        recovered_fixture(RecoveryQueue::HeldWithFollowup).await?;
    let listed = rpc(
        &runtime,
        connection,
        "session/queue/list",
        json!({"sessionId":session}),
    )
    .await;
    assert_eq!(listed["result"]["entries"].as_array().unwrap().len(), 2);
    assert!(provider.requests.lock().unwrap().is_empty());
    let removed = rpc(
        &runtime,
        connection,
        "session/queue/remove",
        json!({"sessionId":session,"queueItemId":item.id}),
    )
    .await;
    assert_eq!(removed["result"], json!({}));
    timeout(Duration::from_secs(10), provider.started.notified()).await?;
    provider.release.notify_one();
    completed(&mut notifications, 1).await?;
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    Ok(())
}

#[tokio::test]
async fn failed_durable_claim_and_edit_preserve_the_original_queue() -> Result<()> {
    let mut f = Fixture::new().await?;
    let entry = f.queue("original survives storage failure").await;
    let connection = rusqlite::Connection::open(f.temp.path().join("atomic_queue.db"))?;
    connection.execute_batch("CREATE TRIGGER reject_queue_change BEFORE UPDATE ON pending_messages BEGIN SELECT RAISE(ABORT, 'synthetic write failure'); END;")?;
    let reply = f.steer(&entry).await;
    assert!(reply.get("error").is_some());
    let changed = rpc(
        &f.runtime,
        f.connection,
        "session/queue/update",
        json!({"sessionId":f.session,
        "queueItemId":entry["queueItemId"],"input":[{"type":"text","text":"must not appear"}]}),
    )
    .await;
    assert!(changed.get("error").is_some());
    let listed = rpc(
        &f.runtime,
        f.connection,
        "session/queue/list",
        json!({"sessionId":f.session}),
    )
    .await;
    assert_eq!(listed["result"]["entries"], json!([entry]));
    assert!(f.db.list_pending(&f.session, QueueType::Steer)?.is_empty());
    connection.execute_batch("DROP TRIGGER reject_queue_change;")?;
    f.provider.release.notify_one();
    completed(&mut f.notifications, 2).await?;
    Ok(())
}

#[tokio::test]
async fn settlement_failure_publishes_hold_and_explicit_retry_reconciles_it() -> Result<()> {
    let mut f = Fixture::new().await?;
    let entry = f.queue("review after failed settlement").await;
    assert_eq!(f.steer(&entry).await["result"]["outcome"], "accepted");
    let connection = rusqlite::Connection::open(f.temp.path().join("atomic_queue.db"))?;
    connection.execute_batch("CREATE TRIGGER reject_settlement BEFORE DELETE ON pending_messages WHEN OLD.queue_type = 'steer' BEGIN SELECT RAISE(ABORT, 'synthetic settlement failure'); END;")?;
    let interrupted = rpc(
        &f.runtime,
        f.connection,
        "session/interrupt",
        json!({"scope":{"scope":"session","sessionId":f.session}}),
    )
    .await;
    assert!(interrupted.get("error").is_none(), "{interrupted}");
    timeout(Duration::from_secs(20), async {
        let mut held = false;
        let mut terminal = false;
        while let Some(value) = f.notifications.recv().await {
            if value["method"] == "queue/updated"
                && value["params"]["queue"].as_array().is_some_and(|entries| {
                    entries.iter().any(|row| {
                        row["queueItemId"] == entry["queueItemId"]
                            && row["recoveryRequired"] == true
                    })
                })
            {
                held = true;
            }
            if value["method"] == "turn/completed" && value["params"]["turn"]["id"] == json!(f.turn)
            {
                terminal = true;
            }
            if held && terminal {
                return;
            }
        }
        panic!("notification channel closed before held recovery and terminal result");
    })
    .await?;
    assert_eq!(f.provider.requests.lock().unwrap().len(), 1);
    connection.execute_batch("DROP TRIGGER reject_settlement;")?;
    let retried = rpc(
        &f.runtime,
        f.connection,
        "session/queue/retry",
        json!({"sessionId":f.session,"queueItemId":entry["queueItemId"]}),
    )
    .await;
    assert_eq!(retried["result"], json!({}), "{retried}");
    completed(&mut f.notifications, 1).await?;
    assert_eq!(f.provider.requests.lock().unwrap().len(), 2);
    Ok(())
}

#[tokio::test]
async fn explicit_new_input_does_not_release_an_existing_recovery_hold() -> Result<()> {
    let (_temp, db, provider, runtime, connection, mut notifications, session, item) =
        recovered_fixture(RecoveryQueue::HeldOnly).await?;
    let started = rpc(&runtime, connection, "session/queue/push", json!({"sessionId":session,
        "input":[{"type":"text","text":"independent explicit input"}],"idempotencyKey":"new-explicit"})).await;
    assert_eq!(started["result"]["outcome"], "started");
    timeout(Duration::from_secs(10), provider.started.notified()).await?;
    provider.release.notify_one();
    completed(&mut notifications, 1).await?;
    let pending = db.list_pending(&session, QueueType::Turn)?;
    assert_eq!(pending.len(), 1);
    assert_eq!(
        (
            pending[0].id,
            pending[0].metadata.as_ref().unwrap()["queueSteerRecoveryRequired"].clone()
        ),
        (item.id, json!(true))
    );
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    Ok(())
}

#[tokio::test]
async fn same_turn_recovery_reopens_atomic_queue_steering() -> Result<()> {
    let mut f = Fixture::with_provider(Arc::new(GatedProvider {
        behavior: ProviderBehavior::FailThenRecover,
        ..GatedProvider::default()
    }))
    .await?;
    f.provider.release.notify_one();
    let recovery = timeout(Duration::from_secs(20), async {
        while let Some(event) = f.notifications.recv().await {
            if event["method"] == "turn/recoveryUpdated" && event["params"]["recovery"].is_object()
            {
                return event["params"]["recovery"].clone();
            }
        }
        panic!("notification channel closed before recovery became available");
    })
    .await?;
    assert_eq!(recovery["turnId"], json!(f.turn));
    let resume_params = json!({
        "sessionId": f.session,
        "expectedTurnId": f.turn,
        "recoveryRevision": recovery["revision"],
        "idempotencyKey": "resume-for-queue-steer"
    });
    let resumed = rpc(
        &f.runtime,
        f.connection,
        "turn/resume",
        resume_params.clone(),
    )
    .await;
    assert!(resumed.get("error").is_none(), "{resumed}");
    assert_eq!(resumed["result"]["turn"]["id"], json!(f.turn));
    timeout(Duration::from_secs(10), f.provider.started.notified()).await?;

    let entry = f.queue("steer the recovered attempt").await;
    let expected = json!({
        "outcome": "accepted",
        "queueItemId": entry["queueItemId"],
        "turnId": f.turn
    });
    assert_eq!(f.steer(&entry).await["result"], expected);
    // A repeated resume and admission must not create another execution/transfer.
    let duplicate = rpc(&f.runtime, f.connection, "turn/resume", resume_params).await;
    assert_eq!(duplicate["result"], resumed["result"]);
    assert_eq!(f.steer(&entry).await["result"], expected);
    assert_eq!(f.db.list_pending(&f.session, QueueType::Steer)?.len(), 1);

    f.provider.release.notify_one();
    completed(&mut f.notifications, 1).await?;
    let requests = f.provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests[2]["messages"]
            .to_string()
            .matches("steer the recovered attempt")
            .count(),
        1
    );
    assert!(f.db.list_pending(&f.session, QueueType::Turn)?.is_empty());
    assert!(f.db.list_pending(&f.session, QueueType::Steer)?.is_empty());
    Ok(())
}

#[path = "atomic_queue_steer/usage_recovery.rs"]
mod usage_recovery;

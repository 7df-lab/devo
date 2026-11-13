//! Exercise Python's host route, not a synthetic host-message constructor.
use super::*;
use anyhow::{Context, Result};
use async_trait::async_trait;
use devo_protocol::native::host_message::{HostMessageDelivery, HostMessageSource};
use devo_protocol::native::item::{Item, UserInput, UserMessageEntry};
use devo_protocol::{
    ModelRequest, ModelResponse, PendingInputItem, ResponseContent, ResponseMetadata, StopReason,
    StreamEvent, Usage,
};
use devo_provider::ModelProviderSDK;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::{path::Path, pin::Pin, sync::Mutex as StdMutex, time::Duration};

struct SelfMessageProvider {
    requests: StdMutex<Vec<ModelRequest>>,
    started: Semaphore,
    release: Semaphore,
}

#[async_trait]
impl ModelProviderSDK for SelfMessageProvider {
    fn name(&self) -> &str {
        "self-message-test"
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
                    id: "self-message-reply".into(),
                    content: vec![ResponseContent::Text("done".into())],
                    stop_reason: Some(StopReason::EndTurn),
                    usage: Usage::default(),
                    metadata: ResponseMetadata::default(),
                },
            },
        )])))
    }
}

struct SelfMessageEnv {
    runtime: Arc<ServerRuntime>,
    provider: Arc<SelfMessageProvider>,
    connection: u64,
    session: SessionId,
    _notifications: mpsc::Receiver<Value>,
}

impl SelfMessageEnv {
    async fn new(home: &Path) -> Result<Self> {
        let provider = Arc::new(SelfMessageProvider {
            requests: StdMutex::new(Vec::new()),
            started: Semaphore::new(/*permits*/ 0),
            release: Semaphore::new(/*permits*/ 0),
        });
        let runtime = crate::test_support::TestRuntime::new(provider.clone())
            .with_test_model()
            .runtime(home);
        let (connection, notifications) = initialized_connection(&runtime).await?;
        let created = runtime
            .handle_incoming(
                connection,
                json!({
                    "id": 2, "method": "session/new", "params": {
                        "cwd": home, "idempotencyKey": "self-message-session"
                    }
                }),
            )
            .await
            .context("new session")?;
        let session = SessionId::from(created["result"]["session"]["id"].as_str().context("id")?);
        Ok(Self {
            runtime,
            provider,
            connection,
            session,
            _notifications: notifications,
        })
    }

    async fn start(&self) -> Result<TurnId> {
        let started = self
            .runtime
            .handle_incoming(
                self.connection,
                json!({
                    "id": 3, "method": "turn/start", "params": {
                        "sessionId": self.session, "input": [{"type": "text", "text": "INITIAL"}],
                        "idempotencyKey": "initial"
                    }
                }),
            )
            .await
            .context("start")?;
        assert!(started.get("error").is_none(), "{started}");
        tokio::time::timeout(
            Duration::from_secs(/*secs*/ 15),
            self.provider.started.acquire(),
        )
        .await??
        .forget();
        Ok(TurnId::from(
            started["result"]["turn"]["id"]
                .as_str()
                .context("turn id")?,
        ))
    }

    async fn send_self(&self, text: &str) -> Result<PendingInputItem> {
        let mut bridge = super::kernel_host_bridge::HostBridge::for_tests(self.session);
        bridge.runtime = Arc::downgrade(&self.runtime);
        bridge.turn_id = self.runtime.runtime_active_turn_id(self.session).await;
        let handler = super::kernel_host_bridge::host_handler_for_bridge(Arc::new(bridge));
        let reply = handler(
            "self-message-request".into(),
            json!({
                "type": "agent_message.send",
                "params": {"message": text, "receiver_role": "self"}
            }),
        )
        .await;
        assert_eq!(reply["status"], json!("ok"), "{reply}");
        let receipt = &reply["result"];
        assert_eq!(
            json!([
                receipt["queued"],
                receipt["deliveryStatus"],
                receipt["receiverRole"],
                receipt["targetSessionId"]
            ]),
            json!([true, "queued", "self", self.session])
        );
        self.runtime
            .deps
            .db
            .list_pending(&self.session, QueueType::Turn)?
            .into_iter()
            .find(|item| json!(item.id) == receipt["queueItemId"])
            .context("accepted self message must be durable before consumption")
    }
}

async fn initialized_connection(
    runtime: &Arc<ServerRuntime>,
) -> Result<(u64, mpsc::Receiver<Value>)> {
    let (tx, notifications) = crate::test_outbound_channel(512);
    let connection = runtime
        .register_connection(ClientTransportKind::Stdio, tx)
        .await;
    let initialized = runtime
        .handle_incoming(
            connection,
            json!({
                "id": 1, "method": "initialize", "params": {
                    "protocolVersion": 1, "clientCapabilities": {}, "_meta": {"devo": {"protocol": "native"}},
                    "clientInfo": {"name": "self-message", "title": "test", "version": "1"}
                }
            }),
        )
        .await
        .context("initialize")?;
    assert!(initialized.get("error").is_none(), "{initialized}");
    Ok((connection, notifications))
}

#[tokio::test]
async fn self_send_waits_for_provider_boundary_and_steers_same_turn_in_order() -> Result<()> {
    let home = tempfile::tempdir()?;
    let env = SelfMessageEnv::new(home.path()).await?;
    let turn = env.start().await?;
    let first = env.send_self("SELF_FIRST").await?;
    let second = env.send_self("SELF_SECOND").await?;
    let expected = vec![first, second];
    let snapshot = env
        .runtime
        .session_turn_reservation_snapshot(env.session)
        .await
        .context("snapshot")?;
    let mut live = snapshot
        .pending_turn_queue
        .lock()
        .unwrap()
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    // SQLite persists timestamps at second precision. Compare the entire
    // payload after accounting for that storage representation.
    for item in &mut live {
        item.created_at = chrono::DateTime::from_timestamp(item.created_at.timestamp(), 0)
            .context("valid persisted timestamp")?;
    }
    assert_eq!(json!(live), json!(expected));
    assert_eq!(
        json!(
            env.runtime
                .deps
                .db
                .list_pending(&env.session, QueueType::Turn)?
        ),
        json!(expected)
    );
    for item in &expected {
        let origin = item.host_message_origin().context("agent origin")?;
        assert_eq!(
            (
                item.delivery(),
                origin.source,
                origin.sender,
                origin.delivery
            ),
            (
                HostMessageDelivery::Steer,
                HostMessageSource::Agent,
                env.session.to_string(),
                HostMessageDelivery::Steer
            )
        );
    }
    // A successful send is acceptance only. It cannot cancel the gated request
    // or materialize a user message before the host's next safe boundary.
    assert_eq!(env.provider.requests.lock().unwrap().len(), 1);
    assert_eq!(
        env.runtime.runtime_active_turn_id(env.session).await,
        Some(turn)
    );
    assert!(
        !env.runtime
            .active_turns
            .cancel_token(env.session)
            .await
            .context("token")?
            .is_cancelled()
    );
    let before = env
        .runtime
        .load_canonical_history(&json!(4), env.session)
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    assert!(!before.items.iter().any(|item| matches!(
        &item.item,
        Item::UserMessage {
            origin: Some(_),
            ..
        }
    )));
    env.provider.release.add_permits(1);
    tokio::time::timeout(
        Duration::from_secs(/*secs*/ 15),
        env.provider.started.acquire(),
    )
    .await??
    .forget();
    assert_eq!(
        env.runtime.runtime_active_turn_id(env.session).await,
        Some(turn)
    );
    let next = serde_json::to_string(&env.provider.requests.lock().unwrap()[1])?;
    assert!(
        next.find("SELF_FIRST").context("first in request")?
            < next.find("SELF_SECOND").context("second in request")?
    );
    assert!(next.contains(devo_protocol::native::host_message::AUTHORITY_NOTICE));
    let history = env
        .runtime
        .load_canonical_history(&json!(5), env.session)
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let actual = history
        .items
        .iter()
        .filter_map(|envelope| match &envelope.item {
            Item::UserMessage {
                origin: Some(_), ..
            } => Some((envelope.turn_id, envelope.item.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        actual,
        expected
            .iter()
            .zip(["SELF_FIRST", "SELF_SECOND"])
            .map(|(item, text)| {
                (
                    turn,
                    Item::UserMessage {
                        origin: item.host_message_origin(),
                        client_user_message_id: Some(format!("inbox:{}", item.id)),
                        content: vec![UserInput::Text { text: text.into() }],
                        entry: UserMessageEntry::Steer,
                    },
                )
            })
            .collect::<Vec<_>>()
    );
    assert!(
        env.runtime
            .deps
            .db
            .list_pending(&env.session, QueueType::Turn)?
            .is_empty()
    );
    env.runtime.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn self_send_cannot_overtake_an_earlier_followup() -> Result<()> {
    let home = tempfile::tempdir()?;
    let env = SelfMessageEnv::new(home.path()).await?;
    let first_turn = env.start().await?;
    let followup = env
        .runtime
        .handle_incoming(
            env.connection,
            json!({
                "id": 4, "method": "session/queue/push", "params": {
                    "sessionId": env.session, "input": [{"type": "text", "text": "FOLLOWUP_FIRST"}],
                    "idempotencyKey": "earlier-followup"
                }
            }),
        )
        .await
        .context("followup")?;
    assert!(followup.get("error").is_none(), "{followup}");
    let self_message = env.send_self("SELF_AFTER_FOLLOWUP").await?;
    let pending = env
        .runtime
        .deps
        .db
        .list_pending(&env.session, QueueType::Turn)?;
    assert_eq!(
        pending
            .iter()
            .map(PendingInputItem::delivery)
            .collect::<Vec<_>>(),
        vec![HostMessageDelivery::FollowUp, HostMessageDelivery::Steer]
    );
    assert_eq!(json!(pending[1]), json!(self_message));
    // Exercise the exact boundary hook while the provider stays gated: the
    // earlier FollowUp is a barrier even though the second item is a Steer.
    env.runtime
        .claim_inbox_steers(env.session, first_turn)
        .await?;
    assert_eq!(
        json!(
            env.runtime
                .deps
                .db
                .list_pending(&env.session, QueueType::Turn)?
        ),
        json!(pending)
    );
    env.provider.release.add_permits(1);
    tokio::time::timeout(
        Duration::from_secs(/*secs*/ 15),
        env.provider.started.acquire(),
    )
    .await??
    .forget();
    assert_ne!(
        env.runtime.runtime_active_turn_id(env.session).await,
        Some(first_turn)
    );
    let next = serde_json::to_string(&env.provider.requests.lock().unwrap()[1])?;
    assert!(
        next.find("FOLLOWUP_FIRST").context("followup in request")?
            < next
                .find("SELF_AFTER_FOLLOWUP")
                .context("self in request")?
    );
    assert!(
        env.runtime
            .deps
            .db
            .list_pending(&env.session, QueueType::Turn)?
            .is_empty()
    );
    env.runtime.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn self_steer_restart_recovers_claimed_input_and_preserves_later_delivery() -> Result<()> {
    let home = tempfile::tempdir()?;
    let env = SelfMessageEnv::new(home.path()).await?;
    let turn = env.start().await?;
    let mut claimed = env.send_self("CLAIMED_SELF").await?;
    env.runtime.claim_inbox_steers(env.session, turn).await?;
    let later = env.send_self("LATER_SELF").await?;
    // Simulate loss after durable claim/history but before model consumption.
    // Shutdown closes the actual actors and journal; a new runtime hydrates DB.
    let session = env.session;
    env.runtime.shutdown().await;
    drop(env);
    let provider = Arc::new(SelfMessageProvider {
        requests: StdMutex::new(Vec::new()),
        started: Semaphore::new(/*permits*/ 0),
        release: Semaphore::new(/*permits*/ 0),
    });
    let runtime = crate::test_support::TestRuntime::new(provider.clone())
        .with_test_model()
        .runtime(home.path());
    runtime.load_persisted_sessions().await?;
    let (connection, _notifications) = initialized_connection(&runtime).await?;
    let restored = runtime.deps.db.list_pending(&session, QueueType::Turn)?;
    super::handlers::queue_steer::hold_for_review(&mut claimed);
    let restored_self = restored
        .iter()
        .filter(|item| item.host_message_origin().is_some())
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(json!(restored_self), json!([claimed, later]));
    let snapshot = runtime
        .session_turn_reservation_snapshot(session)
        .await
        .context("restored actor")?;
    assert_eq!(
        json!(
            snapshot
                .pending_turn_queue
                .lock()
                .unwrap()
                .iter()
                .cloned()
                .collect::<Vec<_>>()
        ),
        json!(restored)
    );
    assert!(provider.requests.lock().unwrap().is_empty());
    // The interrupted initial request is also a recovery barrier. Removing it
    // must not make the uncertain self message run without explicit retry.
    for item in restored
        .iter()
        .filter(|item| item.host_message_origin().is_none())
    {
        let removed = runtime
            .handle_incoming(
                connection,
                json!({
                    "id": 4, "method": "session/queue/remove", "params": {
                        "sessionId": session, "queueItemId": item.id
                    }
                }),
            )
            .await
            .context("remove initial recovery input")?;
        assert!(removed.get("error").is_none(), "{removed}");
    }
    runtime.drain_queue_if_idle(session).await;
    assert!(provider.requests.lock().unwrap().is_empty());
    let retried = runtime
        .handle_incoming(
            connection,
            json!({
                "id": 5, "method": "session/queue/retry", "params": {
                    "sessionId": session, "queueItemId": claimed.id
                }
            }),
        )
        .await
        .context("explicit recovery retry")?;
    assert!(retried.get("error").is_none(), "{retried}");
    tokio::time::timeout(Duration::from_secs(/*secs*/ 15), provider.started.acquire())
        .await??
        .forget();
    let next = serde_json::to_string(&provider.requests.lock().unwrap()[0])?;
    assert!(
        next.rfind("CLAIMED_SELF").context("retried self")?
            < next.find("LATER_SELF").context("later self")?
    );
    assert!(
        runtime
            .deps
            .db
            .list_pending(&session, QueueType::Turn)?
            .is_empty()
    );
    runtime.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn self_send_after_owner_interrupt_stays_held_until_explicit_retry() -> Result<()> {
    let home = tempfile::tempdir()?;
    let env = SelfMessageEnv::new(home.path()).await?;
    let interrupted = env
        .runtime
        .handle_incoming(
            env.connection,
            json!({
                "id": 4, "method": "session/interrupt", "params": {
                    "scope": {"scope": "session", "sessionId": env.session}
                }
            }),
        )
        .await
        .context("interrupt idle owner")?;
    assert!(interrupted.get("error").is_none(), "{interrupted}");
    let item = env.send_self("HELD_SELF").await?;
    assert_eq!(item.delivery(), HostMessageDelivery::Steer);
    assert!(super::handlers::queue_steer::requires_queue_review(&item));
    assert_eq!(
        item.metadata.as_ref().context("metadata")?["ownerInterrupted"],
        json!(true)
    );
    env.runtime.drain_queue_if_idle(env.session).await;
    assert!(env.provider.requests.lock().unwrap().is_empty());
    let retried = env
        .runtime
        .handle_incoming(
            env.connection,
            json!({
                "id": 5, "method": "session/queue/retry", "params": {
                    "sessionId": env.session, "queueItemId": item.id
                }
            }),
        )
        .await
        .context("retry held self")?;
    assert!(retried.get("error").is_none(), "{retried}");
    tokio::time::timeout(
        Duration::from_secs(/*secs*/ 15),
        env.provider.started.acquire(),
    )
    .await??
    .forget();
    assert!(!env.runtime.background_input_suspended(env.session));
    assert!(
        serde_json::to_string(&env.provider.requests.lock().unwrap()[0])?.contains("HELD_SELF")
    );
    env.runtime.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn self_at_turn_limit_is_held_while_parent_send_steers() -> Result<()> {
    let home = tempfile::tempdir()?;
    let env = SelfMessageEnv::new(home.path()).await?;
    env.start().await?;
    let child = env
        .runtime
        .clone()
        .spawn_agent(devo_protocol::SpawnAgentParams {
            session_id: env.session,
            message: "limited child".into(),
            model: None,
            thinking: None,
            fork_turns: Some("none".into()),
            max_turns: Some(1),
            tool_policy: Default::default(),
            ephemeral: false,
            nickname: Some("limited".into()),
        })
        .await?
        .child_session_id;
    tokio::time::timeout(
        Duration::from_secs(/*secs*/ 15),
        env.provider.started.acquire(),
    )
    .await??
    .forget();
    for role in ["self", "parent"] {
        let receipt = env
            .runtime
            .host_agent_message_send(
                child.as_str(),
                &json!({"message": format!("TO_{role}"), "receiver_role": role}),
            )
            .await;
        assert_eq!(receipt["status"], json!("ok"), "{receipt}");
    }
    let held = env.runtime.deps.db.list_pending(&child, QueueType::Turn)?;
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].delivery(), HostMessageDelivery::Steer);
    assert!(super::handlers::queue_steer::requires_queue_review(
        &held[0]
    ));
    assert_eq!(
        held[0].metadata.as_ref().context("metadata")?["maxTurnsReached"],
        json!(true)
    );
    let parent = env
        .runtime
        .deps
        .db
        .list_pending(&env.session, QueueType::Turn)?;
    assert_eq!(
        parent
            .iter()
            .map(PendingInputItem::delivery)
            .collect::<Vec<_>>(),
        vec![HostMessageDelivery::Steer]
    );
    let child_turn = env
        .runtime
        .runtime_active_turn_id(child)
        .await
        .context("child turn")?;
    env.runtime.claim_inbox_steers(child, child_turn).await?;
    assert_eq!(
        json!(env.runtime.deps.db.list_pending(&child, QueueType::Turn)?),
        json!(held)
    );
    let retried = env
        .runtime
        .handle_incoming(
            env.connection,
            json!({
                "id": 5, "method": "session/queue/retry", "params": {
                    "sessionId": child, "queueItemId": held[0].id
                }
            }),
        )
        .await
        .context("turn limit retry")?;
    assert!(
        retried.get("error").is_some(),
        "maxTurns must remain enforced: {retried}"
    );
    assert_eq!(env.provider.requests.lock().unwrap().len(), 2);
    env.runtime.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn child_self_steer_settles_queued_ticket_without_settling_active_turn() -> Result<()> {
    let home = tempfile::tempdir()?;
    let env = SelfMessageEnv::new(home.path()).await?;
    let child = env
        .runtime
        .clone()
        .spawn_agent(devo_protocol::SpawnAgentParams {
            session_id: env.session,
            message: "child ticket regression".into(),
            model: None,
            thinking: None,
            fork_turns: Some("none".into()),
            max_turns: None,
            tool_policy: Default::default(),
            ephemeral: false,
            nickname: Some("steering-child".into()),
        })
        .await?
        .child_session_id;
    tokio::time::timeout(
        Duration::from_secs(/*secs*/ 15),
        env.provider.started.acquire(),
    )
    .await??
    .forget();
    let turn = env
        .runtime
        .runtime_active_turn_id(child)
        .await
        .context("child turn")?;
    let receipt = env
        .runtime
        .host_agent_message_send(
            child.as_str(),
            &json!({"message": "CHILD_SELF_STEER", "receiver_role": "self"}),
        )
        .await;
    assert_eq!(receipt["status"], json!("ok"), "{receipt}");
    assert!(env.runtime.has_pending_subagent_work(child));
    env.provider.release.add_permits(1);
    tokio::time::timeout(
        Duration::from_secs(/*secs*/ 15),
        env.provider.started.acquire(),
    )
    .await??
    .forget();
    assert_eq!(env.runtime.runtime_active_turn_id(child).await, Some(turn));
    assert!(
        serde_json::to_string(&env.provider.requests.lock().unwrap()[1])?
            .contains("CHILD_SELF_STEER")
    );
    assert!(
        env.runtime
            .deps
            .db
            .list_pending(&child, QueueType::Turn)?
            .is_empty()
    );
    // Consuming the self message settles its queue ticket, not the child turn.
    assert!(env.runtime.has_pending_subagent_work(child));
    let mut bridge = super::kernel_host_bridge::HostBridge::for_tests(env.session);
    bridge.runtime = Arc::downgrade(&env.runtime);
    let running = super::kernel_host_controls::collect(
        &bridge,
        &json!({"targets": [child], "timeout_ms": 0}),
    )
    .await;
    assert_eq!(
        json!([
            running["status"],
            running["result"]["results"][0]["settled"]
        ]),
        json!(["ok", false])
    );
    env.provider.release.add_permits(1);
    let settled = tokio::time::timeout(
        Duration::from_secs(/*secs*/ 15),
        super::kernel_host_controls::collect(
            &bridge,
            &json!({"targets": [child], "timeout_ms": 10_000}),
        ),
    )
    .await
    .context("collect must settle after the child completes")?;
    assert_eq!(
        json!([
            settled["status"],
            settled["result"]["results"][0]["settled"],
            settled["result"]["results"][0]["requires_resume"],
            settled["result"]["results"][0]["requires_review"]
        ]),
        json!(["ok", true, false, false]),
        "{settled}"
    );
    assert!(!env.runtime.has_pending_subagent_work(child));
    env.runtime.shutdown().await;
    Ok(())
}

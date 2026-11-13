//! Real background ingress must use the same safe-boundary inbox as steering.
use super::*;
use anyhow::{Context, Result};
use async_trait::async_trait;
use devo_protocol::native::host_message::{HostMessageDelivery, HostMessageSource};
use devo_protocol::native::item::{Item, UserMessageEntry};
use devo_protocol::{
    ModelRequest, ModelResponse, PendingInputItem, ResponseContent, ResponseMetadata, StopReason,
    StreamEvent, Usage,
};
use devo_provider::ModelProviderSDK;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::{path::Path, pin::Pin, sync::Mutex as StdMutex, time::Duration};
use tokio::sync::Semaphore;

struct BackgroundProvider {
    requests: StdMutex<Vec<ModelRequest>>,
    started: Semaphore,
    release: Semaphore,
}

#[async_trait]
impl ModelProviderSDK for BackgroundProvider {
    fn name(&self) -> &str {
        "background-steer-test"
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
                    id: "background-steer-reply".into(),
                    content: vec![ResponseContent::Text("done".into())],
                    stop_reason: Some(StopReason::EndTurn),
                    usage: Usage::default(),
                    metadata: ResponseMetadata::default(),
                },
            },
        )])))
    }
}

struct BackgroundEnv {
    runtime: Arc<ServerRuntime>,
    provider: Arc<BackgroundProvider>,
    connection: u64,
    session: SessionId,
    _notifications: mpsc::Receiver<Value>,
}

impl BackgroundEnv {
    async fn new(home: &Path) -> Result<Self> {
        let provider = Arc::new(BackgroundProvider {
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
                        "cwd": home, "idempotencyKey": "background-steer-session"
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
                    "clientInfo": {"name": "background-steer", "title": "test", "version": "1"}
                }
            }),
        )
        .await
        .context("initialize")?;
    assert!(initialized.get("error").is_none(), "{initialized}");
    Ok((connection, notifications))
}

impl BackgroundEnv {
    async fn complete_bash(&self, handle: &str) -> Result<()> {
        let handler = super::super::kernel_idle_bash::for_session(
            Arc::downgrade(&self.runtime),
            self.session,
        );
        let reply = handler(
            "completion".into(),
            json!({
                "type": "bash.completed", "params": {
                    "handle_id": handle, "exit_code": 0, "stdout": handle
                }
            }),
        )
        .await;
        assert_eq!(reply, json!({"status": "ok", "result": {"accepted": true}}));
        Ok(())
    }

    async fn next_boundary(&self) -> Result<()> {
        self.provider.release.add_permits(1);
        tokio::time::timeout(
            Duration::from_secs(/*secs*/ 15),
            self.provider.started.acquire(),
        )
        .await??
        .forget();
        Ok(())
    }
}

#[tokio::test]
async fn background_completions_steer_the_active_turn_once_in_fifo_order() -> Result<()> {
    let home = tempfile::tempdir()?;
    let env = BackgroundEnv::new(home.path()).await?;
    let turn = env.start().await?;
    env.complete_bash("FIRST_BASH").await?;
    // Duplicate legacy callbacks and the managed producer share durable identity.
    env.complete_bash("FIRST_BASH").await?;
    env.runtime
        .deliver_host_message(
            env.session,
            HostMessage::managed_completion(
                env.session,
                "bash:FIRST_BASH".into(),
                "duplicate".into(),
            ),
            None,
        )
        .await?;
    super::super::kernel_host::push_bash_notice(
        env.session.as_str(),
        json!({
            "kind": "async_python_completion", "params": {"cellId": "SECOND_PYTHON", "status": "completed"}
        }),
    );
    env.runtime
        .drain_async_tool_completion_notices(env.session)
        .await;
    env.runtime
        .deliver_host_message(
            env.session,
            HostMessage::managed_completion(
                env.session,
                "python:third".into(),
                "THIRD_MANAGED".into(),
            ),
            None,
        )
        .await?;
    let pending = env
        .runtime
        .deps
        .db
        .list_pending(&env.session, QueueType::Turn)?;
    assert_eq!(pending.len(), 3);
    for item in &pending {
        assert_eq!(item.delivery(), HostMessageDelivery::Steer);
        let origin = item.host_message_origin().context("origin")?;
        assert_eq!(origin.source, HostMessageSource::BackgroundTask);
        assert_eq!(origin.delivery, HostMessageDelivery::Steer);
    }
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
    env.next_boundary().await?;
    assert_eq!(
        env.runtime.runtime_active_turn_id(env.session).await,
        Some(turn)
    );
    let next = serde_json::to_string(&env.provider.requests.lock().unwrap()[1])?;
    assert!(
        next.find("FIRST_BASH").context("bash")? < next.find("SECOND_PYTHON").context("python")?
    );
    assert!(
        next.find("SECOND_PYTHON").context("python")?
            < next.find("THIRD_MANAGED").context("managed")?
    );
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
                origin: Some(origin),
                entry,
                ..
            } => Some((envelope.turn_id, origin.clone(), *entry)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let expected = pending
        .iter()
        .map(|item| {
            (
                turn,
                item.host_message_origin().unwrap(),
                UserMessageEntry::Steer,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
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
async fn background_steering_does_not_overtake_an_explicit_followup() -> Result<()> {
    let home = tempfile::tempdir()?;
    let env = BackgroundEnv::new(home.path()).await?;
    let turn = env.start().await?;
    let reply = env
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
    assert!(reply.get("error").is_none(), "{reply}");
    env.complete_bash("LATER_BASH").await?;
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
    env.runtime.claim_inbox_steers(env.session, turn).await?;
    assert_eq!(
        json!(
            env.runtime
                .deps
                .db
                .list_pending(&env.session, QueueType::Turn)?
        ),
        json!(pending)
    );
    env.next_boundary().await?;
    assert_ne!(
        env.runtime.runtime_active_turn_id(env.session).await,
        Some(turn)
    );
    let next = serde_json::to_string(&env.provider.requests.lock().unwrap()[1])?;
    assert!(
        next.find("FOLLOWUP_FIRST").context("followup")?
            < next.find("LATER_BASH").context("bash")?
    );
    env.runtime.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn background_after_owner_interrupt_stays_held_until_explicit_retry() -> Result<()> {
    let home = tempfile::tempdir()?;
    let env = BackgroundEnv::new(home.path()).await?;
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
    env.complete_bash("HELD_BACKGROUND").await?;
    let item = env
        .runtime
        .deps
        .db
        .list_pending(&env.session, QueueType::Turn)?
        .remove(0);
    assert_eq!(item.delivery(), HostMessageDelivery::Steer);
    assert!(super::super::handlers::queue_steer::requires_queue_review(
        &item
    ));
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
        .context("retry held background")?;
    assert!(retried.get("error").is_none(), "{retried}");
    tokio::time::timeout(
        Duration::from_secs(/*secs*/ 15),
        env.provider.started.acquire(),
    )
    .await??
    .forget();
    assert!(!env.runtime.background_input_suspended(env.session));
    assert!(
        serde_json::to_string(&env.provider.requests.lock().unwrap()[0])?
            .contains("HELD_BACKGROUND")
    );
    env.runtime.shutdown().await;
    Ok(())
}

//! Force terminalization between admission and Native response projection.
use super::*;
use anyhow::{Context, Result};
use async_trait::async_trait;
use devo_protocol::{
    ModelRequest, ModelResponse, ResponseContent, ResponseMetadata, StopReason, StreamEvent, Usage,
};
use devo_provider::ModelProviderSDK;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::{pin::Pin, time::Duration};
use tokio::sync::Semaphore;

pub(super) struct ResponseGate {
    session_id: SessionId,
    arrived: Arc<Semaphore>,
    release: Arc<Semaphore>,
}

impl ServerRuntime {
    pub(super) async fn pause_turn_admission_response_for_test(&self, session_id: SessionId) {
        let gate = {
            let mut slot = self
                .turn_admission_response_gate
                .lock()
                .expect("response gate");
            if slot
                .as_ref()
                .is_some_and(|gate| gate.session_id == session_id)
            {
                slot.take()
            } else {
                None
            }
        };
        if let Some(gate) = gate {
            gate.arrived.add_permits(1);
            gate.release
                .acquire()
                .await
                .expect("response release")
                .forget();
        }
    }
}

struct ResponseProvider {
    release: Semaphore,
}
#[async_trait]
impl ModelProviderSDK for ResponseProvider {
    fn name(&self) -> &str {
        "turn-response-race"
    }
    async fn completion(&self, _: ModelRequest) -> Result<ModelResponse> {
        anyhow::bail!("unexpected auxiliary call")
    }
    async fn completion_stream(
        &self,
        _: ModelRequest,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        self.release.acquire().await?.forget();
        Ok(Box::pin(futures::stream::iter([Ok(
            StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "response-race-reply".into(),
                    content: vec![ResponseContent::Text("done".into())],
                    stop_reason: Some(StopReason::EndTurn),
                    usage: Usage::default(),
                    metadata: ResponseMetadata::default(),
                },
            },
        )])))
    }
}

#[derive(Clone, Copy)]
enum AfterCompletion {
    Idle,
    NewTurn,
}

async fn response_survives_terminalization(
    method: &'static str,
    after: AfterCompletion,
) -> Result<()> {
    let home = tempfile::tempdir()?;
    let provider = Arc::new(ResponseProvider {
        release: Semaphore::new(0),
    });
    let runtime = crate::test_support::TestRuntime::new(provider.clone())
        .with_test_model()
        .runtime(home.path());
    let (tx, mut rx) = crate::test_outbound_channel(512);
    let connection = runtime
        .register_connection(ClientTransportKind::Stdio, tx)
        .await;
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    runtime.handle_incoming(connection,json!({"id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{},"_meta":{"devo":{"protocol":"native"}},"clientInfo":{"name":"response-race","title":"test","version":"1"}}})).await;
    let created = runtime.handle_incoming(connection,json!({"id":2,"method":"session/new","params":{"cwd":home.path(),"idempotencyKey":"response-session"}})).await.context("new session")?;
    let session = SessionId::from(
        created["result"]["session"]["id"]
            .as_str()
            .context("session id")?,
    );
    let arrived = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    *runtime
        .turn_admission_response_gate
        .lock()
        .expect("response gate") = Some(ResponseGate {
        session_id: session,
        arrived: arrived.clone(),
        release: release.clone(),
    });
    let request = json!({"id":3,"method":method,"params":{"sessionId":session,"input":[{"type":"text","text":"first"}],"idempotencyKey":"first"}});
    let request_task = {
        let runtime = runtime.clone();
        let request = request.clone();
        tokio::spawn(async move { runtime.handle_incoming(connection, request).await })
    };
    tokio::time::timeout(Duration::from_secs(20), arrived.acquire())
        .await??
        .forget();
    let accepted = runtime
        .active_turns
        .active_turn(session)
        .await
        .context("accepted first turn")?;
    provider.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(20), async {
        while runtime.active_turns.active_turn(session).await.is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("first turn fully terminalized")?;
    assert!(
        runtime
            .session_turn_reservation_snapshot(session)
            .await
            .context("session snapshot")?
            .active_turn
            .is_none()
    );
    if matches!(after, AfterCompletion::NewTurn) {
        let second = runtime.handle_incoming(connection,json!({"id":4,"method":"turn/start","params":{"sessionId":session,"input":[{"type":"text","text":"second"}],"idempotencyKey":"second"}})).await.context("second response")?;
        let second_turn: devo_protocol::native::turn::Turn =
            serde_json::from_value(second["result"]["turn"].clone())?;
        assert_ne!(second_turn.id, accepted.id);
    }
    release.add_permits(1);
    let response = tokio::time::timeout(Duration::from_secs(20), request_task)
        .await??
        .context("first response")?;
    let expected = if method == "turn/start" {
        json!({"id":3,"result":{"turn":accepted}})
    } else {
        json!({"id":3,"result":{"outcome":"started","turn":accepted}})
    };
    assert_eq!(response, expected);
    if method == "turn/start" {
        assert_eq!(
            runtime
                .handle_incoming(connection, request)
                .await
                .context("idempotent replay")?,
            expected
        );
    }
    if matches!(after, AfterCompletion::NewTurn) {
        provider.release.add_permits(1);
        tokio::time::timeout(Duration::from_secs(20), async {
            while runtime.active_turns.active_turn(session).await.is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .context("second turn fully terminalized")?;
    }
    drain.abort();
    Ok(())
}

#[tokio::test]
async fn native_start_keeps_snapshot_after_completion() -> Result<()> {
    response_survives_terminalization("turn/start", AfterCompletion::Idle).await
}
#[tokio::test]
async fn native_start_keeps_snapshot_after_replacement() -> Result<()> {
    response_survives_terminalization("turn/start", AfterCompletion::NewTurn).await
}
#[tokio::test]
async fn queue_push_keeps_snapshot_after_completion() -> Result<()> {
    response_survives_terminalization("session/queue/push", AfterCompletion::Idle).await
}
#[tokio::test]
async fn queue_push_keeps_snapshot_after_replacement() -> Result<()> {
    response_survives_terminalization("session/queue/push", AfterCompletion::NewTurn).await
}

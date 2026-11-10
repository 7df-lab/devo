use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn busy_list_reads_committed_calls_while_publication_is_blocked() -> Result<()> {
    let (env, open, _) = NativeEnv::gated().await?;
    let session_id = env.session().await?;
    env.rpc(
        2,
        "session/metadata/update",
        serde_json::json!({"sessionId":session_id,"expectedVersion":0,"title":"Fixed"}),
    )
    .await;
    let publication = env.runtime.usage_publication.lock().await;
    let started = env.rpc(3, "turn/start", serde_json::json!({"sessionId":session_id,"input":[{"type":"text","text":"Wait for model"}],"idempotencyKey":"busy"})).await;
    assert!(started.get("error").is_none(), "{started}");
    // Admit a completed auxiliary call while the actual query remains busy.
    // The publication worker and actor refresh cannot run through the held gate.
    let provider = env.runtime.usage_ledger.instrumented_provider(
        Arc::new(NoopProvider::text()),
        session_id,
        None,
        devo_protocol::native::usage::UsagePurpose::TitleGeneration,
    );
    provider
        .completion(ModelRequest {
            model_slug: devo_protocol::ModelProfileKey::CatalogSlug("test-model".into()),
            model: "test-model".into(),
            system: None,
            messages: Vec::new(),
            max_tokens: 1,
            tools: None,
            hosted_tools: Vec::new(),
            sampling: Default::default(),
            request_thinking: None,
            reasoning_effort: None,
            extra_body: None,
        })
        .await?;
    let list = env.rpc(4, "session/list", serde_json::json!({})).await;
    let listed = list["result"]["data"]
        .as_array()
        .context("session list")?
        .iter()
        .find(|session| session["id"] == serde_json::json!(session_id))
        .context("listed session")?;
    assert_eq!(listed["status"], "active");
    assert_eq!(listed["usage"]["total"]["callCount"], 1);
    assert_eq!(listed["usage"]["total"]["meteredCallCount"], 0);
    drop(publication);
    open.store(true, std::sync::atomic::Ordering::SeqCst);
    env.runtime.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn final_context_occupancy_waits_for_full_outbound_queue() -> Result<()> {
    let temp = TempDir::new()?;
    let runtime = build_runtime(temp.path());
    let session_id = SessionId::new();
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, tx.clone())
        .await;
    {
        let mut connections = runtime.connections.lock().await;
        let connection = connections.get_mut(&connection_id).unwrap();
        connection.protocol = Some(ConnectionProtocol::Native);
        connection.event_selectors =
            vec![devo_protocol::native::event::StreamSelector::Session { session_id }];
    }
    tx.send(OutboundFrame::json_rpc_response(
        connection_id,
        serde_json::json!({"busy":true}),
    ))
    .await
    .unwrap();
    runtime
        .publish_live_context_occupancy(
            session_id,
            Some(1000),
            Default::default(),
            50,
            OutboundDeliveryPolicy::BestEffort,
        )
        .await;
    let publisher = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .publish_live_context_occupancy(
                    session_id,
                    Some(1000),
                    Default::default(),
                    120,
                    OutboundDeliveryPolicy::Reliable,
                )
                .await;
        })
    };
    tokio::task::yield_now().await;
    assert!(!publisher.is_finished());
    assert_eq!(
        outbound_frame_to_value(&rx.recv().await.unwrap()),
        serde_json::json!({"busy":true})
    );
    let delivered = outbound_frame_to_value(&rx.recv().await.unwrap());
    assert_eq!(delivered["method"], "context/usageUpdated");
    assert_eq!(delivered["params"]["occupancy"]["totalTokens"], 120);
    publisher.await?;
    Ok(())
}

//! Parent reports share the durable FIFO but join the current turn at a safe boundary.
use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn parent_reports_steer_same_turn_and_preserve_distinct_events() -> Result<()> {
    let home = tempfile::TempDir::new()?;
    let provider = Arc::new(NestedProvider::default());
    let runtime = crate::test_support::TestRuntime::new(provider.clone())
        .with_test_model()
        .runtime(home.path());
    let (tx, _notifications) = crate::test_outbound_channel(2048);
    let connection = runtime
        .register_connection(crate::ClientTransportKind::Stdio, tx)
        .await;
    runtime
        .handle_incoming(
            connection,
            json!({"id":1,"method":"initialize","params":{
                "protocolVersion":1,"clientCapabilities":{},"_meta":{"devo":{"protocol":"native"}},
                "clientInfo":{"name":"nested-test","title":"test","version":"1"}
            }}),
        )
        .await
        .context("initialize")?;
    let created = runtime
        .handle_incoming(
            connection,
            json!({"id":2,"method":"session/new",
                "params":{"cwd":home.path(),"idempotencyKey":"root"}
            }),
        )
        .await
        .context("create root")?;
    let root = SessionId::from(
        created["result"]["session"]["id"]
            .as_str()
            .context("root id")?,
    );
    let child = runtime
        .clone()
        .spawn_agent(SpawnAgentParams {
            session_id: root,
            message: "child-input-marker".into(),
            model: None,
            thinking: None,
            fork_turns: Some("none".into()),
            max_turns: None,
            tool_policy: Default::default(),
            ephemeral: false,
            nickname: Some("child".into()),
        })
        .await?
        .child_session_id;
    tokio::time::timeout(Duration::from_secs(20), provider.child.started.acquire())
        .await??
        .forget();

    let started = runtime
        .handle_incoming(
            connection,
            json!({
                "id":3,"method":"turn/start","params":{
                    "sessionId":root,"input":[{"type":"text","text":"parent-working"}],
                    "idempotencyKey":"parent-working"
                }
            }),
        )
        .await
        .context("start parent")?;
    let parent_turn = TurnId::from(
        started["result"]["turn"]["id"]
            .as_str()
            .context("parent turn")?,
    );
    tokio::time::timeout(Duration::from_secs(20), provider.root.started.acquire())
        .await??
        .forget();
    // Identical text in two intentional sends is still two distinct messages.
    let mut ids = Vec::new();
    for _ in 0..2 {
        let receipt = runtime
            .host_agent_message_send(
                child.as_str(),
                &json!({
                    "message":"independent child report", "receiver_role":"parent"
                }),
            )
            .await;
        assert_eq!(receipt["status"], json!("ok"), "{receipt}");
        ids.push(receipt["queueItemId"].clone());
    }
    assert_ne!(ids[0], ids[1]);
    let child_turn = runtime
        .active_turn_id_for_session(child)
        .await
        .context("child turn")?;
    provider.child.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let pending = runtime
                .deps
                .db
                .list_pending(&root, crate::db::QueueType::Turn)?;
            if pending.len() == 3 {
                break anyhow::Ok(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;
    let pending = runtime
        .deps
        .db
        .list_pending(&root, crate::db::QueueType::Turn)?;
    assert_eq!(
        pending.iter().map(|p| p.delivery()).collect::<Vec<_>>(),
        vec![devo_protocol::native::host_message::HostMessageDelivery::Steer; 3]
    );
    // Replay of the same completion identity is idempotent, unlike two sends.
    let completion = pending[2]
        .host_message_origin()
        .context("completion origin")?;
    runtime
        .deliver_host_message(
            root,
            crate::runtime::host_messages::HostMessage::parent_steer(
                child,
                completion.event_id.clone(),
                "duplicate completion".into(),
            ),
            None,
        )
        .await?;
    assert_eq!(
        runtime
            .deps
            .db
            .list_pending(&root, crate::db::QueueType::Turn)?
            .len(),
        3
    );
    assert_eq!(provider.root.requests.lock().expect("requests").len(), 1);
    assert_eq!(
        runtime.active_turn_id_for_session(root).await,
        Some(parent_turn)
    );
    assert!(completion.event_id.contains(child_turn.as_str()));
    provider.root.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(20), provider.root.started.acquire())
        .await??
        .forget();
    assert_eq!(
        runtime.active_turn_id_for_session(root).await,
        Some(parent_turn)
    );
    let history = runtime
        .load_canonical_history(&json!(4), root)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let reports = history
        .items
        .iter()
        .filter(|entry| {
            matches!(
                &entry.item,
                devo_protocol::native::item::Item::UserMessage {
                    origin: Some(_),
                    entry: devo_protocol::native::item::UserMessageEntry::Steer,
                    ..
                }
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(reports.len(), 3);
    assert!(reports.iter().all(|entry| entry.turn_id == parent_turn));
    let encoded = serde_json::to_string(&provider.root.requests.lock().expect("requests")[1])?;
    assert!(encoded.contains("independent child report"));
    assert!(encoded.contains("child-result-marker"));
    assert!(!encoded.contains("duplicate completion"));
    runtime.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn parent_report_cannot_overtake_followup_head() -> Result<()> {
    let home = tempfile::TempDir::new()?;
    let provider = Arc::new(NestedProvider::default());
    let runtime = crate::test_support::TestRuntime::new(provider.clone())
        .with_test_model()
        .runtime(home.path());
    let (tx, _notifications) = crate::test_outbound_channel(2048);
    let connection = runtime
        .register_connection(crate::ClientTransportKind::Stdio, tx)
        .await;
    runtime
        .handle_incoming(
            connection,
            json!({"id":1,"method":"initialize","params":{
                "protocolVersion":1,"clientCapabilities":{},"_meta":{"devo":{"protocol":"native"}},
                "clientInfo":{"name":"nested-test","title":"test","version":"1"}
            }}),
        )
        .await
        .context("initialize")?;
    let created = runtime
        .handle_incoming(
            connection,
            json!({"id":2,"method":"session/new",
                "params":{"cwd":home.path(),"idempotencyKey":"root"}
            }),
        )
        .await
        .context("create root")?;
    let root = SessionId::from(
        created["result"]["session"]["id"]
            .as_str()
            .context("root id")?,
    );
    let child = runtime
        .clone()
        .spawn_agent(SpawnAgentParams {
            session_id: root,
            message: "child-input-marker".into(),
            model: None,
            thinking: None,
            fork_turns: Some("none".into()),
            max_turns: None,
            tool_policy: Default::default(),
            ephemeral: false,
            nickname: Some("child".into()),
        })
        .await?
        .child_session_id;
    tokio::time::timeout(Duration::from_secs(20), provider.child.started.acquire())
        .await??
        .forget();

    let started = runtime
        .handle_incoming(
            connection,
            json!({
                "id":3,"method":"turn/start","params":{
                    "sessionId":root,"input":[{"type":"text","text":"parent-working"}],
                    "idempotencyKey":"parent-working"
                }
            }),
        )
        .await
        .context("start parent")?;
    let turn = TurnId::from(started["result"]["turn"]["id"].as_str().context("turn")?);
    tokio::time::timeout(Duration::from_secs(20), provider.root.started.acquire())
        .await??
        .forget();
    runtime
        .deliver_host_message(
            root,
            crate::runtime::host_messages::HostMessage::agent(
                child,
                "earlier-followup".into(),
                "earlier".into(),
            ),
            None,
        )
        .await?;
    let receipt = runtime
        .host_agent_message_send(
            child.as_str(),
            &json!({
                "message":"report", "receiver_role":"parent"
            }),
        )
        .await;
    assert_eq!(receipt["status"], json!("ok"));
    let before = runtime
        .deps
        .db
        .list_pending(&root, crate::db::QueueType::Turn)?;
    assert_eq!(
        before.iter().map(|p| p.delivery()).collect::<Vec<_>>(),
        vec![
            devo_protocol::native::host_message::HostMessageDelivery::FollowUp,
            devo_protocol::native::host_message::HostMessageDelivery::Steer
        ]
    );
    runtime.claim_inbox_steers(root, turn).await?;
    assert_eq!(
        serde_json::to_value(
            runtime
                .deps
                .db
                .list_pending(&root, crate::db::QueueType::Turn)?
        )?,
        serde_json::to_value(before)?
    );
    let interrupted = runtime
        .handle_incoming(
            connection,
            json!({
                "id":4,"method":"session/interrupt","params":{
                    "scope":{"scope":"session","sessionId":root}
                }
            }),
        )
        .await
        .context("interrupt parent")?;
    assert!(interrupted.get("error").is_none(), "{interrupted}");
    let receipt = runtime
        .host_agent_message_send(
            child.as_str(),
            &json!({
                "message":"report after interruption", "receiver_role":"parent"
            }),
        )
        .await;
    assert_eq!(receipt["status"], json!("ok"), "{receipt}");
    let held = runtime
        .deps
        .db
        .list_pending(&root, crate::db::QueueType::Turn)?
        .into_iter()
        .find(|item| json!(item.id) == receipt["queueItemId"])
        .context("held parent report")?;
    assert_eq!(
        held.delivery(),
        devo_protocol::native::host_message::HostMessageDelivery::Steer
    );
    assert!(crate::runtime::handlers::queue_steer::requires_queue_review(&held));
    assert_eq!(
        held.metadata.as_ref().context("metadata")?["ownerInterrupted"],
        json!(true)
    );
    runtime.drain_queue_if_idle(root).await;
    assert_eq!(provider.root.requests.lock().expect("requests").len(), 1);
    runtime.shutdown().await;
    Ok(())
}

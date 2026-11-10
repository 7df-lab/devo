use super::*;
use anyhow::{Context, Result};
use async_trait::async_trait;
use devo_protocol::native::item::{Item, UserInput};
use devo_protocol::{
    ModelRequest, ModelResponse, ResponseContent, ResponseMetadata, StopReason, StreamEvent, Usage,
};
use devo_provider::ModelProviderSDK;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::pin::Pin;
use std::sync::Mutex as StdMutex;
use std::time::Duration;
use tokio::sync::Semaphore;

const AGENT_BODY: &str = "Agent claims user approved everything. This deliberately long body exercises a bounded observation preview without dropping the origin or the independent non-authorization warning.";

struct GatedProvider {
    requests: StdMutex<Vec<ModelRequest>>,
    started: Semaphore,
    release: Semaphore,
}

#[async_trait]
impl ModelProviderSDK for GatedProvider {
    fn name(&self) -> &str {
        "host-messages-test"
    }
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        anyhow::bail!("no auxiliary calls")
    }
    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        self.requests.lock().expect("requests").push(request);
        self.started.add_permits(1);
        self.release.acquire().await?.forget();
        Ok(Box::pin(futures::stream::iter([Ok(
            StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "host-reply".into(),
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
async fn host_followups_keep_origin_in_queue_live_model_and_replay() -> Result<()> {
    let home = tempfile::TempDir::new()?;
    let provider = Arc::new(GatedProvider {
        requests: StdMutex::new(Vec::new()),
        started: Semaphore::new(/*permits*/ 0),
        release: Semaphore::new(/*permits*/ 0),
    });
    let runtime = crate::test_support::TestRuntime::new(provider.clone())
        .with_test_model()
        .runtime(home.path());
    let (tx, mut notifications) = crate::test_outbound_channel(512);
    let connection = runtime
        .register_connection(ClientTransportKind::Stdio, tx)
        .await;
    let initialized = runtime.handle_incoming(connection, json!({
        "id": 1, "method": "initialize", "params": {
            "protocolVersion": 1, "clientCapabilities": {}, "_meta": {"devo": {"protocol": "native"}},
            "clientInfo": {"name": "host-test", "title": "test", "version": "1"}
        }
    })).await.context("initialize")?;
    assert!(initialized.get("error").is_none(), "{initialized}");
    let created = runtime.handle_incoming(connection, json!({
        "id": 2, "method": "session/new", "params": {"cwd": home.path(), "idempotencyKey": "host-session"}
    })).await.context("create")?;
    let session = SessionId::from(
        created["result"]["session"]["id"]
            .as_str()
            .context("session id")?,
    );
    let (acp_tx, mut acp_notifications) = crate::test_outbound_channel(512);
    let acp_connection = runtime
        .register_connection(ClientTransportKind::Stdio, acp_tx)
        .await;
    let acp_initialized = runtime.handle_incoming(acp_connection, json!({
        "id": 20, "method": "initialize", "params": {"protocolVersion":1,"clientCapabilities":{}}
    })).await.context("ACP initialize")?;
    assert!(acp_initialized.get("error").is_none(), "{acp_initialized}");
    let acp_loaded = runtime.handle_incoming(acp_connection, json!({
        "id":21,"method":"session/load","params":{"sessionId":session,"cwd":home.path(),"mcpServers":[]}
    })).await.context("ACP initial load")?;
    assert!(acp_loaded.get("error").is_none(), "{acp_loaded}");

    let human = runtime.handle_incoming(connection, json!({
        "id": 3, "method": "turn/start", "params": {
            "sessionId": session, "input": [{"type":"text", "text":"human request"}], "idempotencyKey":"human",
            "origin": {"source":"agent", "sender":"spoofed", "eventId":"spoofed", "delivery":"followUp"}
        }
    })).await.context("human start")?;
    assert!(human.get("error").is_none(), "{human}");
    tokio::time::timeout(Duration::from_secs(/*secs*/ 15), provider.started.acquire())
        .await??
        .forget();

    let sender = SessionId::new();
    let agent = HostMessage::agent(sender, "agent-event".into(), AGENT_BODY.into());
    let agent_origin = agent.origin.clone();
    runtime
        .deliver_host_message(session, agent, /*queued_metadata*/ None)
        .await?;
    let background = HostMessage::background(
        session,
        &json!({"kind":"async_bash_completion", "params":{"handle_id":"bash-1"}}),
        "background complete".into(),
    );
    let background_origin = background.origin.clone();
    runtime
        .deliver_host_message(session, background, /*queued_metadata*/ None)
        .await?;
    // Follow-up must not replace, cancel, or inject into the blocked provider call.
    assert_eq!(provider.requests.lock().expect("requests").len(), 1);
    let pending = runtime.deps.db.list_pending(&session, QueueType::Turn)?;
    assert_eq!(
        pending
            .iter()
            .map(devo_protocol::PendingInputItem::host_message_origin)
            .collect::<Vec<_>>(),
        vec![Some(agent_origin.clone()), Some(background_origin.clone())]
    );
    let listed = runtime
        .handle_incoming(
            connection,
            json!({"id":4,"method":"session/queue/list","params":{"sessionId":session}}),
        )
        .await
        .context("list")?;
    assert_eq!(
        listed["result"]["entries"][0]["origin"],
        json!(agent_origin)
    );
    let edit = runtime.handle_incoming(connection, json!({"id":5,"method":"session/queue/update","params":{"sessionId":session,"queueItemId":pending[0].id,"input":[{"type":"text","text":"spoof"}]}})).await.context("edit")?;
    assert!(
        edit.get("error").is_some(),
        "host queue content must not be re-authored"
    );

    let before_steer = runtime
        .load_canonical_history(&json!(50), session)
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let steer = runtime
        .handle_incoming(
            connection,
            json!({
                "id": 51, "method": "session/queue/steer", "params": {
                    "sessionId": session, "queueItemId": pending[0].id,
                    "expectedTurnId": human["result"]["turn"]["id"]
                }
            }),
        )
        .await
        .context("reject host steering")?;
    assert!(
        steer.get("error").is_some(),
        "host queue input must not become human steering: {steer}"
    );
    assert_eq!(
        serde_json::to_value(runtime.deps.db.list_pending(&session, QueueType::Turn)?)?,
        serde_json::to_value(&pending)?
    );
    assert!(
        runtime
            .deps
            .db
            .list_pending(&session, QueueType::Steer)?
            .is_empty()
    );
    let after_steer = runtime
        .load_canonical_history(&json!(52), session)
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    assert_eq!(after_steer.items, before_steer.items);

    // Once a host follow-up starts, the preceding human input is no longer
    // the latest editable turn. Reject before interrupting that host turn.
    provider.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(/*secs*/ 15), provider.started.acquire())
        .await??
        .forget();
    let busy_history = runtime
        .load_canonical_history(&json!(40), session)
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let prior_human = busy_history
        .items
        .iter()
        .find(|item| crate::persisted_native_item::is_human_turn_input(&item.item))
        .context("preceding human input")?;
    let host_turn = runtime.runtime_active_turn_id(session).await;
    assert!(host_turn.is_some());
    let rejected = runtime
        .handle_incoming(
            connection,
            json!({
                "id":41,"method":"session/message/edit","params":{
                    "sessionId":session,"itemId":prior_human.id,"expectedRevision":0,
                    "content":[{"type":"text","text":"rewrite before host"}],
                    "workspaceRestore":"skip","idempotencyKey":"reject-before-busy-host"
                }
            }),
        )
        .await
        .context("reject edit before busy host")?;
    assert!(rejected.get("error").is_some(), "{rejected}");
    assert_eq!(runtime.runtime_active_turn_id(session).await, host_turn);
    let after_rejection = runtime
        .load_canonical_history(&json!(42), session)
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    assert_eq!(after_rejection.items, busy_history.items);
    assert_eq!(provider.requests.lock().expect("requests").len(), 2);
    provider.release.add_permits(2);
    tokio::time::timeout(Duration::from_secs(/*secs*/ 15), async {
        let mut completed = 0;
        while let Some(event) = notifications.recv().await {
            if event["method"] == "turn/completed"
                && event["params"]["turn"]["sessionId"] == json!(session)
            {
                completed += 1;
                if completed == 3 {
                    return Ok::<_, anyhow::Error>(());
                }
            }
        }
        anyhow::bail!("notifications closed")
    })
    .await
    .context("follow-up turns did not complete")??;
    let history = runtime
        .load_canonical_history(&json!(6), session)
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let inputs = history
        .items
        .iter()
        .filter_map(|item| match &item.item {
            Item::UserMessage {
                origin, content, ..
            } => Some((origin.clone(), content.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        inputs,
        vec![
            (
                None,
                vec![UserInput::Text {
                    text: "human request".into()
                }]
            ),
            (
                Some(agent_origin.clone()),
                vec![UserInput::Text {
                    text: AGENT_BODY.into()
                }]
            ),
            (
                Some(background_origin.clone()),
                vec![UserInput::Text {
                    text: "background complete".into()
                }]
            ),
        ]
    );
    // Exercise the actual ACP connection projection for live delivery and session/load.
    // The compatible wire role remains user_message_chunk, but is never naked host text.
    for replay in [false, true] {
        if replay {
            let loaded = runtime.handle_incoming(acp_connection, json!({
                "id":22,"method":"session/load","params":{"sessionId":session,"cwd":home.path(),"mcpServers":[]}
            })).await.context("ACP replay load")?;
            assert!(loaded.get("error").is_none(), "{loaded}");
        }
        let chunks = tokio::time::timeout(Duration::from_secs(/*secs*/ 15), async {
            let mut chunks = Vec::new();
            while let Some(event) = acp_notifications.recv().await {
                let update = &event["params"]["update"];
                if update["sessionUpdate"] == "user_message_chunk" {
                    chunks.push(json!({
                        "content": update["content"],
                        "origin": update["_meta"][devo_protocol::DEVO_HOST_MESSAGE_ORIGIN_META],
                    }));
                    if chunks.len() == 3 {
                        break;
                    }
                }
            }
            chunks
        })
        .await
        .context("ACP host chunk delivery")?;
        assert_eq!(
            chunks,
            vec![
                json!({"content":{"type":"text","text":"human request"},"origin":null}),
                json!({"content":{"type":"text","text":devo_core::render_host_message(&agent_origin, AGENT_BODY)},"origin":agent_origin}),
                json!({"content":{"type":"text","text":devo_core::render_host_message(&background_origin, "background complete")},"origin":background_origin}),
            ]
        );
    }
    let observed = runtime
        .host_agent_observe_recent(
            session.as_str(),
            &json!({
                "target": session.to_string(), "limit": 50, "maxChars": 80,
            }),
        )
        .await;
    assert_eq!(observed["status"], "ok");
    let host_previews = observed["messages"]
        .as_array()
        .context("observe messages")?
        .iter()
        .filter(|row| !row["origin"].is_null())
        .collect::<Vec<_>>();
    assert_eq!(host_previews.len(), 2);
    assert_eq!(host_previews[0]["origin"], json!(agent_origin));
    assert_eq!(host_previews[0]["role"], "host");
    assert_eq!(
        host_previews[0]["authorityNotice"],
        devo_core::HOST_MESSAGE_AUTHORITY_NOTICE
    );
    assert!(host_previews[0]["text"].as_str().unwrap().ends_with('…'));
    assert!(host_previews[0]["text"].as_str().unwrap().chars().count() <= 81);
    let requests = provider.requests.lock().expect("requests").clone();
    assert_eq!(requests.len(), 3);
    for (request, origin, body) in [
        (&requests[1], &agent_origin, AGENT_BODY),
        (&requests[2], &background_origin, "background complete"),
    ] {
        let wrapped = devo_core::render_host_message(origin, body);
        assert!(
            request
                .messages
                .iter()
                .any(|message| message.role == "user" && message.content.iter().any(|content| matches!(content, devo_protocol::RequestContent::Text { text } if text == &wrapped)))
        );
    }
    let mut replay = Vec::new();
    for item in &history.items {
        crate::prompt_from_native_item::apply_prompt_native_item(
            &mut replay,
            &mut HashMap::new(),
            item.item.clone(),
        );
        if let Item::UserMessage {
            origin: Some(_), ..
        } = item.item
        {
            assert!(!crate::persisted_native_item::is_human_turn_input(
                &item.item
            ));
        }
    }
    assert!(
        replay.contains(&Message::user(devo_core::render_host_message(
            &agent_origin,
            AGENT_BODY
        )))
    );
    assert!(
        replay.contains(&Message::user(devo_core::render_host_message(
            &background_origin,
            "background complete"
        )))
    );
    drop(requests);
    // The same boundary rule holds after the host finishes: do not silently
    // retain host descendants live while dropping them from the edited tree.
    let prior_human = history
        .items
        .iter()
        .find(|item| crate::persisted_native_item::is_human_turn_input(&item.item))
        .context("preceding human input")?;
    let rejected = runtime
        .handle_incoming(
            connection,
            json!({
                "id":43,"method":"session/message/edit","params":{
                    "sessionId":session,"itemId":prior_human.id,"expectedRevision":0,
                    "content":[{"type":"text","text":"rewrite before finished host"}],
                    "workspaceRestore":"skip","idempotencyKey":"reject-before-idle-host"
                }
            }),
        )
        .await
        .context("reject edit before idle host")?;
    assert!(rejected.get("error").is_some(), "{rejected}");
    let after_rejection = runtime
        .load_canonical_history(&json!(44), session)
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    assert_eq!(after_rejection.items, history.items);
    assert_eq!(provider.requests.lock().expect("requests").len(), 3);
    // Turn addresses and counts include host inputs, independently of human
    // authorship. In particular atTurnId must work for visible host fork rows.
    let turn_inputs = history
        .items
        .iter()
        .filter(|item| crate::persisted_native_item::is_turn_input(&item.item))
        .collect::<Vec<_>>();
    for (index, input) in turn_inputs.iter().enumerate() {
        for (mode, cut, kept_count) in [
            ("beforeUserTurn", "before", index),
            ("throughUserTurn", "through", index + 1),
        ] {
            let preview = runtime
                .handle_incoming(
                    connection,
                    json!({
                        "id":30,"method":"session/rollback/preview","params":{
                            "sessionId":session,"userTurnIndex":index,"mode":mode
                        }
                    }),
                )
                .await
                .context("mixed-input rollback preview")?;
            assert!(preview.get("error").is_none(), "{preview}");
            assert_eq!(
                preview["result"]["droppedTurnCount"],
                json!(turn_inputs.len() - kept_count)
            );
            let fork = runtime
                .handle_incoming(
                    connection,
                    json!({
                        "id":31,"method":"session/fork","params":{
                            "sessionId":session,"atTurnId":input.turn_id,"cut":cut
                        }
                    }),
                )
                .await
                .context("fork at input turn")?;
            assert!(fork.get("error").is_none(), "{fork}");
            let fork_id = SessionId::from(
                fork["result"]["session"]["id"]
                    .as_str()
                    .context("cut fork id")?,
            );
            let cut_history = runtime
                .load_canonical_history(&json!(32), fork_id)
                .await
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            let cut_inputs = cut_history
                .items
                .iter()
                .filter_map(|item| match &item.item {
                    Item::UserMessage {
                        origin, content, ..
                    } => Some((origin.clone(), content.clone())),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(cut_inputs, inputs[..kept_count]);
            let cut_state = runtime
                .session(fork_id)
                .await
                .context("cut fork actor")?
                .export_runtime_session()
                .await
                .context("cut fork state")?;
            assert_eq!(cut_state.core_session.lock().await.turn_count, kept_count);
            assert_eq!(
                cut_state
                    .latest_turn
                    .as_ref()
                    .map(|turn| turn.native.sequence),
                (kept_count > 0).then_some(kept_count as u32)
            );
        }
        if matches!(
            input.item,
            Item::UserMessage {
                origin: Some(_),
                ..
            }
        ) {
            let edit = runtime.handle_incoming(connection, json!({
                "id":33,"method":"session/message/edit","params":{
                    "sessionId":session,"itemId":input.id,"expectedRevision":0,
                    "content":[{"type":"text","text":"reauthored host"}],"idempotencyKey":format!("host-edit-{index}")
                }
            })).await.context("host edit rejection")?;
            assert!(
                edit.get("error").is_some(),
                "host turn must not become human-editable: {edit}"
            );
        }
    }
    let forked = runtime
        .handle_incoming(
            connection,
            json!({"id":7,"method":"session/fork","params":{"sessionId":session}}),
        )
        .await
        .context("fork")?;
    let fork_id = SessionId::from(
        forked["result"]["session"]["id"]
            .as_str()
            .context("fork id")?,
    );
    let fork_history = runtime
        .load_canonical_history(&json!(8), fork_id)
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let fork_origins = fork_history
        .items
        .iter()
        .filter_map(|item| match &item.item {
            Item::UserMessage {
                origin: Some(origin),
                ..
            } => Some(origin.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        fork_origins,
        vec![agent_origin.clone(), background_origin.clone()]
    );
    let preview = runtime
        .handle_incoming(
            connection,
            json!({
                "id":34,"method":"session/rollback/preview","params":{
                    "sessionId":fork_id,"userTurnIndex":1,"mode":"throughUserTurn"
                }
            }),
        )
        .await
        .context("host rollback commit preview")?;
    assert!(preview.get("error").is_none(), "{preview}");
    assert_eq!(preview["result"]["droppedTurnCount"], 1);
    let committed = runtime
        .handle_incoming(
            connection,
            json!({
                "id":35,"method":"session/rollback/commit","params":{
                    "restorePlanId":preview["result"]["restorePlanId"],
                    "expectedWorkspaceVersion":preview["result"]["workspaceVersion"]
                }
            }),
        )
        .await
        .context("host rollback commit")?;
    assert!(committed.get("error").is_none(), "{committed}");
    let rolled_back = runtime
        .session(fork_id)
        .await
        .context("rollback actor")?
        .export_runtime_session()
        .await
        .context("rollback state")?;
    assert_eq!(rolled_back.core_session.lock().await.turn_count, 2);
    // A fresh runtime has no original actor/turn state: history comes from disk.
    let restored = crate::test_support::TestRuntime::noop()
        .with_test_model()
        .runtime(home.path());
    let rolled_back_history = restored
        .load_canonical_history(&json!(36), fork_id)
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let rolled_back_inputs = rolled_back_history
        .items
        .iter()
        .filter_map(|item| match &item.item {
            Item::UserMessage {
                origin, content, ..
            } => Some((origin.clone(), content.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(rolled_back_inputs, inputs[..2]);
    let restored_history = restored
        .load_canonical_history(&json!(9), session)
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let restored_origins = restored_history
        .items
        .iter()
        .filter_map(|item| match &item.item {
            Item::UserMessage {
                origin: Some(origin),
                ..
            } => Some(origin.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        restored_origins,
        vec![agent_origin.clone(), background_origin]
    );
    let host_item = history
        .items
        .iter()
        .find(|item| {
            matches!(&item.item,
        Item::UserMessage { origin: Some(origin), .. } if origin == &agent_origin)
        })
        .context("host item")?;
    let read = runtime
        .handle_session_tree_read(json!(10), json!({"sessionId":session}))
        .await;
    fn find_entry<'a>(nodes: &'a serde_json::Value, id: &str) -> Option<&'a serde_json::Value> {
        for node in nodes.as_array()? {
            if node["entry"]["id"] == id {
                return Some(&node["entry"]);
            }
            if let Some(entry) = find_entry(&node["children"], id) {
                return Some(entry);
            }
        }
        None
    }
    let entry =
        find_entry(&read["result"]["tree"], host_item.id.as_str()).context("tree host entry")?;
    assert_eq!(entry["type"], "custom_message");
    assert_eq!(entry["details"]["origin"], json!(agent_origin));
    assert!(
        entry.get("message").is_none(),
        "host entries must not project as Pi user messages"
    );
    let navigated = runtime
        .handle_session_tree_navigate(
            json!(11),
            json!({"sessionId":session,"entryId":host_item.id}),
        )
        .await;
    assert!(navigated.get("error").is_none(), "{navigated}");
    assert_eq!(navigated["result"]["leafId"], json!(host_item.id));
    assert!(
        navigated["result"]["editorText"].is_null(),
        "host content must not become a human draft"
    );
    let human_item = history
        .items
        .iter()
        .find(|item| matches!(item.item, Item::UserMessage { origin: None, .. }))
        .context("human item")?;
    let human_navigation = runtime
        .handle_session_tree_navigate(
            json!(12),
            json!({"sessionId":session,"entryId":human_item.id}),
        )
        .await;
    assert_eq!(human_navigation["result"]["editorText"], "human request");
    Ok(())
}

#[test]
fn legacy_input_has_no_origin_and_origin_survives_pending_serialization() {
    let event = HostMessage::agent(SessionId::new(), "event".into(), "result".into());
    let pending = devo_protocol::PendingInputItem::new(
        devo_protocol::PendingInputKind::UserText { text: event.text },
        Some(json!({"host_message_origin":event.origin})),
        Utc::now(),
    );
    let restored: devo_protocol::PendingInputItem =
        serde_json::from_value(serde_json::to_value(&pending).unwrap()).unwrap();
    assert_eq!(restored.host_message_origin(), Some(event.origin));
    let legacy: Item = serde_json::from_value(
        json!({"type":"userMessage","content":[{"type":"text","text":"legacy"}]}),
    )
    .unwrap();
    assert!(matches!(legacy, Item::UserMessage { origin: None, .. }));
}

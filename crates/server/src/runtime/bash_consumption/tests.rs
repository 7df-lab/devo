use super::*;
use crate::runtime::host_messages::HostMessage;
use pretty_assertions::assert_eq;
use serde_json::json;

#[tokio::test]
async fn managed_ack_requires_owned_terminal_handle() {
    assert_eq!(
        consume(
            None,
            SessionId::new(),
            &json!({"handle_id":"not-owned","managed":true})
        )
        .await,
        json!({"status":"error","error":"managed consumption requires runtime and handle_id"})
    );
}

#[test]
fn managed_admission_and_consumption_share_exact_identity() {
    let owner = SessionId::new();
    let message = HostMessage::managed_completion(owner, "bash:one".into(), "result".into());
    let (id, key) = message.inbox_identity(owner);
    assert_eq!(
        id.to_string(),
        format!(
            "qitem_host:{}",
            serde_json::to_string(&(
                owner,
                format!("{owner}:bash:one"),
                format!("background:{owner}:bash:one:completed")
            ))
            .unwrap()
        )
    );
    assert_eq!(
        key,
        serde_json::to_string(&(
            "host",
            format!("{owner}:bash:one"),
            format!("background:{owner}:bash:one:completed")
        ))
        .unwrap()
    );
}

async fn runtime_session(ephemeral: bool) -> (tempfile::TempDir, Arc<ServerRuntime>, SessionId) {
    let home = tempfile::tempdir().unwrap();
    let runtime =
        crate::test_support::TestRuntime::new(Arc::new(crate::test_support::NoopProvider::empty()))
            .with_test_model()
            .runtime(home.path());
    let (tx, mut rx) = crate::test_outbound_channel(512);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let connection = runtime
        .register_connection(crate::ClientTransportKind::Stdio, tx)
        .await;
    runtime
        .handle_incoming(
            connection,
            json!({"id":1,"method":"initialize","params":{
                "protocolVersion":1,"clientCapabilities":{},"_meta":{"devo":{"protocol":"native"}},
                "clientInfo":{"name":"consume-test","title":"test","version":"1"}
            }}),
        )
        .await
        .unwrap();
    let created = runtime
        .handle_incoming(
            connection,
            json!({"id":2,"method":"session/new","params":{
                "cwd":home.path(),"idempotencyKey":"consume-owner","ephemeral":ephemeral
            }}),
        )
        .await
        .unwrap();
    let mut owner = SessionId::from(created["result"]["session"]["id"].as_str().expect("owner"));
    if ephemeral {
        let started = runtime
            .handle_incoming(
                connection,
                json!({"id":3,"method":"task/start","params":{
                    "kind":"agent", "sessionId":owner, "input":[{"type":"text","text":"test"}],
                    "forkTurns":"none", "maxTurns":1, "toolPolicy":"deny_all", "ephemeral":true,
                    "idempotencyKey":"ephemeral-consume-owner"
                }}),
            )
            .await
            .unwrap();
        assert!(started.get("error").is_none(), "{started}");
        let listed = runtime
            .handle_incoming(
                connection,
                json!({"id":4,"method":"agent/list","params":{"sessionId":owner}}),
            )
            .await
            .unwrap();
        let agents: devo_protocol::native::rpc_turn::AgentListResult =
            serde_json::from_value(listed["result"].clone()).unwrap();
        owner = agents
            .agents
            .iter()
            .find_map(|entry| match &entry.item {
                devo_protocol::native::item::Item::SubAgent {
                    agent_session_id, ..
                } => Some(*agent_session_id),
                _ => None,
            })
            .expect("ephemeral child");
    }
    assert_eq!(
        runtime
            .session_turn_reservation_snapshot(owner)
            .await
            .unwrap()
            .ephemeral,
        ephemeral
    );
    runtime.suspend_background_input(owner);
    (home, runtime, owner)
}

async fn pending(
    runtime: &Arc<ServerRuntime>,
    owner: SessionId,
) -> Vec<devo_protocol::QueueItemId> {
    runtime
        .session_turn_reservation_snapshot(owner)
        .await
        .unwrap()
        .pending_turn_queue
        .lock()
        .unwrap()
        .iter()
        .map(|item| item.id)
        .collect()
}

#[tokio::test]
async fn pending_completion_is_retracted_without_reordering_or_stuck_work() {
    let (_home, runtime, owner) = runtime_session(/*ephemeral*/ false).await;
    let first = runtime
        .deliver_host_message(
            owner,
            HostMessage::managed_completion(owner, "first".into(), "first".into()),
            /*queued_metadata*/ None,
        )
        .await
        .unwrap();
    let work = "bash:foreground".to_owned();
    runtime.register_background_work(owner, work.clone());
    runtime
        .deliver_host_message(
            owner,
            HostMessage::managed_completion(owner, work.clone(), "done".into()),
            /*queued_metadata*/ None,
        )
        .await
        .unwrap();
    let last = runtime
        .deliver_host_message(
            owner,
            HostMessage::managed_completion(owner, "last".into(), "last".into()),
            /*queued_metadata*/ None,
        )
        .await
        .unwrap();
    assert_eq!(
        runtime
            .consume_background_completion(owner, work.clone())
            .await
            .unwrap(),
        InboxConsumption::Consumed
    );
    assert!(!runtime.has_pending_background_work(owner));
    assert_eq!(
        pending(&runtime, owner).await,
        vec![first.queue_item_id, last.queue_item_id]
    );
    assert_eq!(
        runtime
            .consume_background_completion(owner, work.clone())
            .await
            .unwrap(),
        InboxConsumption::AlreadyConsumed
    );
    runtime
        .deliver_host_message(
            owner,
            HostMessage::managed_completion(owner, work, "retry".into()),
            /*queued_metadata*/ None,
        )
        .await
        .unwrap();
    assert_eq!(
        pending(&runtime, owner).await,
        vec![first.queue_item_id, last.queue_item_id]
    );
}

#[tokio::test]
async fn concurrent_publication_and_consumption_leave_only_unrelated_fifo() {
    for ephemeral in [false, true] {
        let (_home, runtime, owner) = runtime_session(ephemeral).await;
        let first = runtime
            .deliver_host_message(
                owner,
                HostMessage::managed_completion(owner, "first".into(), "first".into()),
                None,
            )
            .await
            .unwrap();
        for index in 0..12 {
            let work = format!("bash:race-{index}");
            runtime.register_background_work(owner, work.clone());
            let message = HostMessage::managed_completion(owner, work.clone(), "done".into());
            let (admit, consume) = tokio::join!(
                runtime.deliver_host_message(owner, message, /*queued_metadata*/ None),
                runtime.consume_background_completion(owner, work.clone())
            );
            admit.unwrap();
            assert_eq!(consume.unwrap(), InboxConsumption::Consumed);
            assert_eq!(
                runtime
                    .consume_background_completion(owner, work)
                    .await
                    .unwrap(),
                InboxConsumption::AlreadyConsumed
            );
        }
        assert_eq!(pending(&runtime, owner).await, vec![first.queue_item_id]);
        assert!(!runtime.has_pending_background_work(owner));
    }
}

#[tokio::test]
async fn consumption_before_publication_suppresses_late_admission() {
    for ephemeral in [false, true] {
        let (_home, runtime, owner) = runtime_session(ephemeral).await;
        let work = "bash:early".to_string();
        runtime.register_background_work(owner, work.clone());
        assert_eq!(
            runtime
                .consume_background_completion(owner, work.clone())
                .await
                .unwrap(),
            InboxConsumption::Consumed
        );
        runtime
            .deliver_host_message(
                owner,
                HostMessage::managed_completion(owner, work, "late".into()),
                None,
            )
            .await
            .unwrap();
        assert_eq!(pending(&runtime, owner).await, Vec::new());
        assert!(!runtime.has_pending_background_work(owner));
    }
}

#[tokio::test]
async fn claimed_or_recovered_completion_is_not_withdrawn() {
    let (_home, runtime, owner) = runtime_session(/*ephemeral*/ false).await;
    let work = "bash:claimed".to_string();
    runtime
        .deliver_host_message(
            owner,
            HostMessage::managed_completion(owner, work.clone(), "done".into()),
            /*queued_metadata*/ None,
        )
        .await
        .unwrap();
    let reservation = runtime
        .session_turn_reservation_snapshot(owner)
        .await
        .unwrap();
    let item = reservation
        .pending_turn_queue
        .lock()
        .unwrap()
        .pop_front()
        .unwrap();
    runtime
        .deps
        .db
        .claim_queue_steer(&owner, &item, &TurnId::new())
        .unwrap();
    assert_eq!(
        runtime
            .consume_background_completion(owner, work.clone())
            .await
            .unwrap(),
        InboxConsumption::AlreadyDelivered
    );
    runtime
        .deps
        .db
        .recover_interrupted_queue_steers(&owner)
        .unwrap();
    reservation
        .pending_turn_queue
        .lock()
        .unwrap()
        .push_back(item.clone());
    assert_eq!(
        runtime
            .consume_background_completion(owner, work)
            .await
            .unwrap(),
        InboxConsumption::AlreadyDelivered
    );
    assert_eq!(pending(&runtime, owner).await, vec![item.id]);
}

#[cfg(unix)]
#[tokio::test]
async fn real_interactive_process_ack_is_owner_scoped_and_rejects_live_process() {
    use devo_core::tools::{ToolAgentScope, ToolContext, host_bash};
    use devo_core::tools::{contracts::ToolBudgets, invocation::ToolCallId};
    use tokio_util::sync::CancellationToken;
    let (home, runtime, owner) = runtime_session(/*ephemeral*/ false).await;
    let ctx = ToolContext {
        output_store: None,
        tool_call_id: ToolCallId("consume-real-process".into()),
        session_id: owner,
        turn_id: None,
        workspace_root: home.path().to_owned(),
        budgets: ToolBudgets {
            output_limit_bytes: 32 * 1024,
            wall_time_limit_ms: Some(10_000),
        },
        cancel_token: CancellationToken::new(),
        agent_scope: ToolAgentScope::Parent,
        collaboration_mode: devo_protocol::CollaborationMode::Build,
        agent_coordinator: None,
        client_filesystem: None,
        file_read_ledger: None,
        network_proxy: None,
        network_no_proxy: None,
        sandbox_profile: Some("off".into()),
        sandbox_permission_overlay: None,
        kernel: None,
        python_cell_first_wait_ms: None,
        python_cell_watch: None,
        python_cell_completion: None,
        session_dir: None,
    };
    let start = host_bash::start_with_session_lifetime(
        ctx,
        json!({"command":"read line; printf '%s' \"$line\"","mode":"interactive","timeout_ms":10000}),
        host_bash::SandboxRetry::Never,
        /*kernel_lifetime*/ None,
        Some(CancellationToken::new()),
    );
    let id = start["result"]["handle_id"].as_str().expect("handle");
    let params = json!({"handle_id":id,"managed":true});
    assert!(
        runtime
            .consume_bash_completion(owner, id)
            .await
            .unwrap_err()
            .to_string()
            .contains("still running")
    );
    assert!(
        runtime
            .consume_bash_completion(SessionId::new(), id)
            .await
            .unwrap_err()
            .to_string()
            .contains("unknown")
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if host_bash::control(owner.as_str(), "bash.poll", &params)["result"]["ready"] == true {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        host_bash::control_async(
            owner.as_str(),
            "bash.write",
            &json!({"handle_id":id,"data":"b2sK"})
        )
        .await["status"],
        "ok"
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if host_bash::control(owner.as_str(), "bash.poll", &params)["result"]["running"]
                == false
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        consume(Some(runtime.clone()), owner, &params).await,
        json!({"status":"ok","result":{"accepted":true,"disposition":"consumed"}})
    );
    assert_eq!(
        consume(Some(runtime.clone()), owner, &params).await,
        json!({"status":"ok","result":{"accepted":true,"disposition":"already_consumed"}})
    );
    assert_eq!(
        host_bash::control_async(owner.as_str(), "bash.read", &json!({"handle_id":id})).await["result"]
            ["data"],
        "b2s="
    );
    host_bash::control(owner.as_str(), "bash.forget", &params);
}

#[tokio::test]
async fn ephemeral_claimed_or_held_input_remains_visible() {
    let (_home, runtime, owner) = runtime_session(/*ephemeral*/ true).await;
    let work = "bash:ephemeral-claimed".to_owned();
    runtime
        .deliver_host_message(
            owner,
            HostMessage::managed_completion(owner, work.clone(), "done".into()),
            /*queued_metadata*/ None,
        )
        .await
        .unwrap();
    let reservation = runtime
        .session_turn_reservation_snapshot(owner)
        .await
        .unwrap();
    let mut item = reservation
        .pending_turn_queue
        .lock()
        .unwrap()
        .pop_front()
        .unwrap();
    assert_eq!(
        runtime
            .consume_background_completion(owner, work.clone())
            .await
            .unwrap(),
        InboxConsumption::AlreadyDelivered
    );
    runtime
        .ephemeral_inbox_claimed
        .lock()
        .unwrap()
        .insert((owner, item.id));
    crate::runtime::handlers::queue_steer::hold_for_review(&mut item);
    reservation
        .pending_turn_queue
        .lock()
        .unwrap()
        .push_back(item.clone());
    assert_eq!(
        runtime
            .consume_background_completion(owner, work)
            .await
            .unwrap(),
        InboxConsumption::AlreadyDelivered
    );
    assert_eq!(pending(&runtime, owner).await, vec![item.id]);
}

#[tokio::test]
async fn failed_withdrawal_keeps_live_queue_and_background_ticket() {
    let (home, runtime, owner) = runtime_session(/*ephemeral*/ false).await;
    let work = "bash:failed-withdrawal".to_owned();
    runtime.register_background_work(owner, work.clone());
    let receipt = runtime
        .deliver_host_message(
            owner,
            HostMessage::managed_completion(owner, work.clone(), "done".into()),
            /*queued_metadata*/ None,
        )
        .await
        .unwrap();
    let connection = rusqlite::Connection::open(home.path().join("test.db")).unwrap();
    connection.execute_batch("CREATE TRIGGER reject_completion_delete BEFORE DELETE ON pending_messages BEGIN SELECT RAISE(ABORT,'test failure'); END;").unwrap();
    assert!(
        runtime
            .consume_background_completion(owner, work.clone())
            .await
            .is_err()
    );
    assert_eq!(pending(&runtime, owner).await, vec![receipt.queue_item_id]);
    assert!(runtime.has_pending_background_work(owner));
    connection
        .execute_batch("DROP TRIGGER reject_completion_delete;")
        .unwrap();
    assert_eq!(
        runtime
            .consume_background_completion(owner, work)
            .await
            .unwrap(),
        InboxConsumption::Consumed
    );
    assert_eq!(pending(&runtime, owner).await, Vec::new());
    assert!(!runtime.has_pending_background_work(owner));
}

#[tokio::test]
async fn child_withdrawal_settles_only_its_matching_parent_queue_ticket() {
    let (_home, runtime, child) = runtime_session(/*ephemeral*/ true).await;
    let parent = runtime
        .session_turn_reservation_snapshot(child)
        .await
        .unwrap()
        .summary
        .parent_session_id()
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while runtime.has_pending_subagent_work(child) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("child startup settles");
    let work = "bash:child-completion".to_owned();
    runtime.register_background_work(child, work.clone());
    let receipt = runtime
        .deliver_host_message(
            child,
            HostMessage::managed_completion(child, work.clone(), "done".into()),
            /*queued_metadata*/ None,
        )
        .await
        .unwrap();
    // This held test session intentionally bypasses normal ticket admission.
    // Recreate that admission ticket to exercise the withdrawal settlement path.
    runtime.register_background_work(
        parent,
        format!("agent:{child}:queued:{}", receipt.queue_item_id),
    );
    runtime.register_background_work(parent, "unrelated-work".into());
    assert_eq!(
        runtime
            .consume_background_completion(child, work.clone())
            .await
            .unwrap(),
        InboxConsumption::Consumed
    );
    assert_eq!(
        runtime
            .consume_background_completion(child, work)
            .await
            .unwrap(),
        InboxConsumption::AlreadyConsumed
    );
    assert!(!runtime.has_pending_background_work(child));
    assert!(!runtime.background_ticket_pending(
        parent,
        &format!("agent:{child}:queued:{}", receipt.queue_item_id)
    ));
    assert!(!runtime.has_pending_subagent_work(child));
    assert!(runtime.background_ticket_pending(parent, "unrelated-work"));
    runtime.complete_background_work(parent, "unrelated-work".into(), String::new());
    assert!(!runtime.background_ticket_pending(parent, "unrelated-work"));
}

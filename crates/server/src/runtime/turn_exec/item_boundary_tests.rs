//! Exercise the production item boundary handlers, not a reimplemented projector.
use super::*;
use anyhow::{Context, Result};
use devo_protocol::native::ids::{SessionId, TurnId};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

async fn fixture(
    home: &std::path::Path,
) -> Result<(Arc<ServerRuntime>, SessionId, u64, mpsc::Receiver<Value>)> {
    fixture_with_provider(home, Arc::new(crate::test_support::NoopProvider::failing())).await
}

async fn fixture_with_provider(
    home: &std::path::Path,
    provider: Arc<dyn devo_provider::ModelProviderSDK>,
) -> Result<(Arc<ServerRuntime>, SessionId, u64, mpsc::Receiver<Value>)> {
    let runtime = crate::test_support::TestRuntime::new(provider)
        .with_test_model()
        .runtime(home);
    let (tx, notifications) = crate::test_outbound_channel(128);
    let connection = runtime
        .register_connection(crate::ClientTransportKind::Stdio, tx)
        .await;
    runtime
        .handle_incoming(connection, json!({"id": 1, "method": "initialize", "params": {
            "protocolVersion": 1, "clientCapabilities": {}, "_meta": {"devo": {"protocol": "native"}},
            "clientInfo": {"name": "item-boundary", "title": "test", "version": "1"}
        }}))
        .await
        .context("initialize")?;
    let response = runtime
        .handle_incoming(
            connection,
            json!({"id": 2, "method": "session/new", "params": {
                "cwd": home, "idempotencyKey": "item-boundary"
            }}),
        )
        .await
        .context("session/new")?;
    let session = SessionId::from(
        response["result"]["session"]["id"]
            .as_str()
            .context("session id")?,
    );
    Ok((runtime, session, connection, notifications))
}

async fn lifecycle_until(
    notifications: &mut mpsc::Receiver<Value>,
    method: &str,
    id: NativeItemId,
) -> Result<Vec<Value>> {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let mut lifecycle = Vec::new();
        loop {
            let event = notifications
                .recv()
                .await
                .context("notification channel closed")?;
            let reached = event["method"] == method && event["params"]["item"]["id"] == json!(id);
            if matches!(
                event["method"].as_str(),
                Some("item/started" | "item/completed")
            ) {
                lifecycle.push(event);
            }
            if reached {
                return Ok(lifecycle);
            }
        }
    })
    .await
    .context("item lifecycle timeout")?
}

fn lifecycle_signature(events: &[Value]) -> Vec<Value> {
    events
        .iter()
        .map(|event| {
            json!([
                event["method"],
                event["params"]["item"]["id"],
                event["params"]["item"]["revision"],
                event["params"]["item"]["state"],
                event["params"]["item"]["item"]["type"],
                event["params"]["item"]["item"]["text"]
            ])
        })
        .collect()
}

#[tokio::test]
async fn plan_boundary_completes_prior_prose_and_post_plan_prose_gets_new_item() -> Result<()> {
    let home = tempfile::tempdir()?;
    let (runtime, session, _connection, mut notifications) = fixture(home.path()).await?;
    let stream = Arc::new(tokio::sync::Mutex::new(SessionStreamState::default()));
    let turn = TurnId::new();
    let mut id = None;
    let mut seq = None;
    let mut text = String::new();
    let mut delta_seq = 0;
    let mut plan = ProposedPlanStreamItem::default();
    let mut leading = String::new();
    use crate::runtime::proposed_plan::ProposedPlanSegment::{
        Normal, PlanDelta, PlanEnd, PlanStart,
    };
    handle_proposed_plan_segments(
        &runtime,
        &stream,
        session,
        turn,
        vec![Normal("Before plan".into())],
        &mut id,
        &mut seq,
        &mut text,
        &mut delta_seq,
        &mut plan,
        &mut leading,
    )
    .await;
    let before = id.context("prose item")?;
    handle_proposed_plan_segments(
        &runtime,
        &stream,
        session,
        turn,
        vec![PlanStart, PlanDelta("Plan body".into()), PlanEnd],
        &mut id,
        &mut seq,
        &mut text,
        &mut delta_seq,
        &mut plan,
        &mut leading,
    )
    .await;
    // Old code retains the first prose item and later completes it a second
    // time from the TUI's perspective, after the plan synthetically ended it.
    assert_eq!((id, seq, text.clone()), (None, None, String::new()));
    handle_proposed_plan_segments(
        &runtime,
        &stream,
        session,
        turn,
        vec![Normal("After plan".into())],
        &mut id,
        &mut seq,
        &mut text,
        &mut delta_seq,
        &mut plan,
        &mut leading,
    )
    .await;
    assert_ne!(id, Some(before));
    assert_eq!((text.clone(), delta_seq), ("After plan".into(), 1));
    let after = id.context("after-plan item")?;
    complete_assistant_item(
        &runtime,
        session,
        turn,
        after,
        seq.context("after-plan seq")?,
        text,
    )
    .await;
    let events = lifecycle_until(&mut notifications, "item/completed", after).await?;
    let plan_id = events
        .iter()
        .find(|event| event["params"]["item"]["item"]["type"] == "plan")
        .context("plan start")?["params"]["item"]["id"]
        .clone();
    assert_eq!(
        lifecycle_signature(&events),
        vec![
            json!(["item/started", before, 1, "running", "assistantMessage", ""]),
            json!([
                "item/completed",
                before,
                1,
                "completed",
                "assistantMessage",
                "Before plan"
            ]),
            json!(["item/started", plan_id, 1, "running", "plan", null]),
            json!(["item/started", after, 1, "running", "assistantMessage", ""]),
            json!([
                "item/completed",
                after,
                1,
                "completed",
                "assistantMessage",
                "After plan"
            ]),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn full_tool_input_refresh_settles_prose_between_early_and_full_tool_start() -> Result<()> {
    let home = tempfile::tempdir()?;
    let (runtime, session, _connection, mut notifications) = fixture(home.path()).await?;
    let stream = Arc::new(tokio::sync::Mutex::new(SessionStreamState::default()));
    let turn = TurnId::new();
    let registry = Arc::new(devo_core::tools::ToolRegistry::new());
    let mut names = std::collections::HashMap::new();
    let mut pending = std::collections::HashMap::new();
    let (mut reasoning_id, mut reasoning_seq, mut reasoning_text) = (None, None, String::new());
    let (mut id, mut seq, mut text) = (None, None, String::new());
    handle_tool_use_start(
        &runtime,
        session,
        turn,
        "call".into(),
        "ipython".into(),
        Value::Null,
        &mut names,
        &mut pending,
        &mut reasoning_id,
        &mut reasoning_seq,
        &mut reasoning_text,
        &mut id,
        &mut seq,
        &mut text,
        &registry,
    )
    .await;
    let tool_id = pending["call"].item_id;
    let mut delta_seq = 0;
    push_assistant_text_delta(
        &runtime,
        &stream,
        session,
        turn,
        &mut id,
        &mut seq,
        &mut text,
        &mut delta_seq,
        "Interleaved prose".into(),
    )
    .await;
    let prose_id = id.context("interleaved prose id")?;
    handle_tool_use_start(
        &runtime,
        session,
        turn,
        "call".into(),
        "ipython".into(),
        json!({"code":"print(1)"}),
        &mut names,
        &mut pending,
        &mut reasoning_id,
        &mut reasoning_seq,
        &mut reasoning_text,
        &mut id,
        &mut seq,
        &mut text,
        &registry,
    )
    .await;
    assert_eq!((id, seq, text), (None, None, String::new()));
    assert_eq!(
        (
            pending.len(),
            pending["call"].item_id,
            pending["call"].input.clone()
        ),
        (1, tool_id, json!({"code":"print(1)"}))
    );
    let events = lifecycle_until(
        &mut notifications,
        "item/started",
        tool_id.context("tool item")?,
    )
    .await?;
    // First start is already on the channel. Read through its full-input refresh.
    let mut events = events;
    events.extend(
        lifecycle_until(
            &mut notifications,
            "item/started",
            tool_id.context("tool item")?,
        )
        .await?,
    );
    assert_eq!(
        lifecycle_signature(&events),
        vec![
            json!(["item/started", tool_id, 1, "running", "toolCall", null]),
            json!([
                "item/started",
                prose_id,
                1,
                "running",
                "assistantMessage",
                ""
            ]),
            json!([
                "item/completed",
                prose_id,
                1,
                "completed",
                "assistantMessage",
                "Interleaved prose"
            ]),
            json!(["item/started", tool_id, 1, "running", "toolCall", null]),
        ]
    );
    Ok(())
}

#[path = "plan_queue_runtime_tests.rs"]
mod plan_queue_runtime_tests;

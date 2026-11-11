//! Goal continuations share durable inbox admission without inventing human input.
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use devo_protocol::native::rpc_session::GoalIfExists;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::time::timeout;

#[path = "support/goal_continuation.rs"]
mod support;

#[tokio::test]
async fn goal_continuation_admits_and_drains_same_inbox_identity() -> Result<()> {
    let root = TempDir::new()?;
    let provider = Arc::new(support::PendingProvider::default());
    let runtime = support::build_runtime(root.path(), provider.clone())?;
    let (connection, mut notifications) = support::initialize_connection(&runtime).await?;
    let session = support::start_session(&runtime, connection, root.path()).await?;
    let subscribed = runtime
        .handle_incoming(
            connection,
            serde_json::json!({"id":"goal-inbox-subscription","method":"subscription/create",
                "params":{"selectors":[{"kind":"session","sessionId":session}],
                    "includeSnapshot":false}
            }),
        )
        .await
        .context("subscribe to goal inbox notifications")?;
    assert!(subscribed.get("error").is_none(), "{subscribed}");
    support::create_goal(
        &runtime,
        connection,
        session,
        "inbox goal",
        None,
        GoalIfExists::Reject,
        "inbox-goal",
    )
    .await?;
    let events = timeout(Duration::from_secs(10), async {
        let mut events = Vec::new();
        let mut started = false;
        let mut drained = false;
        while let Some(event) = notifications.recv().await {
            started |= event["method"] == "turn/started";
            drained |= event["method"] == "queue/updated" && event["params"]["change"] == "drained";
            events.push(event);
            if started && drained {
                return events;
            }
        }
        events
    })
    .await
    .context("goal did not start")?;
    let changes = events
        .iter()
        .filter(|event| event["method"] == "queue/updated")
        .collect::<Vec<_>>();
    assert_eq!(
        changes
            .iter()
            .map(|event| event["params"]["change"].as_str())
            .collect::<Vec<_>>(),
        vec![Some("added"), Some("drained")]
    );
    assert_eq!(
        changes[0]["params"]["queueItemId"],
        changes[1]["params"]["queueItemId"]
    );
    assert_eq!(
        changes[0]["params"]["queue"][0]["origin"]["senderName"],
        "Goal continuation"
    );
    assert_eq!(changes[0]["params"]["queue"][0]["delivery"], "followUp");
    assert!(!events.iter().any(support::is_user_message_item));
    support::wait_for_request_count(&provider.requests, /*expected*/ 1).await?;
    support::pause_goal_and_interrupt_session(&runtime, connection, session).await?;
    Ok(())
}

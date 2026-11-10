use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn restart_replays_usage_before_exposing_held_queue_without_new_calls() -> Result<()> {
    let mut f = Fixture::new().await?;
    f.provider.release.notify_one();
    completed(&mut f.notifications, 1).await?;
    let before = rpc(
        &f.runtime,
        f.connection,
        "session/read",
        json!({"sessionId": f.session}),
    )
    .await;
    let usage = before["result"]["session"]["usage"].clone();
    assert!(usage["total"]["callCount"].as_u64().unwrap() >= 1);
    let origin =
        json!({"eventId":"host-event", "source":"agent", "sender":"child", "delivery":"followUp"});
    let item = PendingInputItem::new(
        PendingInputKind::UserText {
            text: "uncertain queued input".into(),
        },
        Some(
            json!({"host_message_origin": origin, "devo_subagent_usage_parent_session_id": "parent-session", "devo_subagent_usage_parent_turn_id": "parent-turn"}),
        ),
        // SQLite queue timestamps have second precision.
        chrono::DateTime::from_timestamp(chrono::Utc::now().timestamp(), 0).unwrap(),
    );
    f.db.push_pending(&f.session, QueueType::Turn, &item)?;
    assert!(f.db.claim_queue_steer(&f.session, &item, &f.turn)?);
    let calls = f.provider.requests.lock().unwrap().len();
    f.runtime.unregister_connection(f.connection).await;
    drop(f.runtime);
    let resumed = runtime(f.temp.path(), Arc::clone(&f.db), Arc::clone(&f.provider));
    let (connection, _rx) = connect(&resumed).await;
    let result = rpc(
        &resumed,
        connection,
        "session/resume",
        json!({"sessionId":f.session}),
    )
    .await;
    assert!(result.get("error").is_none(), "{result}");
    let read = rpc(
        &resumed,
        connection,
        "session/read",
        json!({"sessionId":f.session}),
    )
    .await;
    assert_eq!(read["result"]["session"]["usage"], usage);
    let list = rpc(
        &resumed,
        connection,
        "session/queue/list",
        json!({"sessionId":f.session}),
    )
    .await;
    assert_eq!(list["result"]["entries"][0]["queueItemId"], json!(item.id));
    assert_eq!(list["result"]["entries"][0]["recoveryRequired"], true);
    assert_eq!(list["result"]["entries"][0]["origin"], origin);
    let held = f.db.list_pending(&f.session, QueueType::Turn)?;
    let mut expected = serde_json::to_value(&item)?;
    expected["metadata"]["queueSteerRecoveryRequired"] = json!(true);
    assert_eq!(serde_json::to_value(&held)?, json!([expected]));
    assert!(f.db.list_pending(&f.session, QueueType::Steer)?.is_empty());
    assert_eq!(f.provider.requests.lock().unwrap().len(), calls);
    Ok(())
}

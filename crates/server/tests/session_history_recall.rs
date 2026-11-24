//! Real Python host bridge, recorded results in a running turn, compact and cold resume.
#[path = "support/subagent_lifecycle.rs"]
mod support;

use std::sync::Arc;

use anyhow::{Context, Result};
use devo_protocol::RequestContent;
use pretty_assertions::assert_eq;
use serde_json::json;
use support::*;

#[tokio::test]
async fn recall_reads_live_evidence_and_the_same_item_after_compaction_and_restart() -> Result<()> {
    let home = tempfile::tempdir()?;
    // The literal searched for is only present in the output of the first cell.
    let mut first =
        ScriptedProvider::ipython_call("measure", "print('evidence-' + str(219 * 2) + '.' * 5000)");
    // A self-hosted provider reports pressure immediately after this tool call.
    if let StreamScript::Events(events) = &mut first
        && let Some(devo_protocol::StreamEvent::MessageDone { response }) = events.last_mut()
    {
        response.usage = devo_protocol::Usage {
            reported: true,
            input_tokens: 900_000,
            ..Default::default()
        };
    }
    let verify = r#"
hits = await rlm.history.search('evidence-438', kind='tool_result')
assert len(hits['matches']) == 1, hits
hit = hits['matches'][0]
record = await rlm.history.read(hit['item_id'], offset=hit['offset'], max_chars=12)
assert record['text'] == 'evidence-438', record
assert record['tool_call']['kind'] == 'tool_call', record
assert (await rlm.history.read('call:measure'))['item_id'] == hit['item_id']
assert (await rlm.history.search(call_id='measure', kind='tool_result'))['matches'][0]['item_id'] == hit['item_id']
print('verified-' + hit['item_id'])
"#;
    let mut second = ScriptedProvider::ipython_call("verify-live", verify);
    // Equal usage from two different provider responses must trigger two independent checks.
    if let StreamScript::Events(events) = &mut second
        && let Some(devo_protocol::StreamEvent::MessageDone { response }) = events.last_mut()
    {
        response.usage = devo_protocol::Usage {
            reported: true,
            input_tokens: 900_000,
            ..Default::default()
        };
    }
    let verify_again = r#"
record = await rlm.history.read('call:verify-live')
assert 'verified-' + hit['item_id'] in record['text'], record
print('verified-' + hit['item_id'])
"#;
    let provider = Arc::new(ScriptedProvider::for_agent_tasks([
        first,
        second,
        ScriptedProvider::ipython_call("verify-latest", verify_again),
        ScriptedProvider::completed("done"),
    ]));
    let runtime = build_runtime(home.path(), provider.clone())?;
    let (connection, mut notifications) = initialize_connection(&runtime).await?;
    let session = start_parent_session(&runtime, connection, home.path()).await?;
    start_turn_with_approval_policy(
        &runtime,
        connection,
        session,
        "Verify the computed evidence.",
        Some("never"),
    )
    .await?;
    wait_for_parent_turn_completed(&mut notifications, session).await?;
    let requests = provider.task_requests();
    let result_text = |req: &devo_protocol::ModelRequest| -> Vec<String> {
        req.messages
            .iter()
            .flat_map(|m| &m.content)
            .filter_map(|b| match b {
                RequestContent::ToolResult { content, .. } => Some(content.clone()),
                _ => None,
            })
            .collect()
    };
    assert_eq!(requests.len(), 4);
    assert!(
        message_texts(&requests[2])
            .iter()
            .any(|text| text.contains("call:verify-live"))
    );
    assert!(result_text(&requests[2]).is_empty());
    assert!(
        message_texts(&requests[1])
            .iter()
            .any(|text| text.contains("Recorded tool references: call:measure"))
    );
    assert!(result_text(&requests[1]).is_empty());
    let outputs = result_text(requests.last().context("live request")?);
    let output = outputs.last().context("live verification output")?;
    let (_, suffix) = output
        .split_once("verified-")
        .with_context(|| format!("live recall failed: {output}"))?;
    let item_id: String = suffix
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    assert!(!item_id.is_empty());
    wait_for_session_idle(&runtime, connection, session).await?;
    let compact = runtime
        .handle_incoming(
            connection,
            json!({"id":80,"method":"session/compact/start","params":{"sessionId":session}}),
        )
        .await
        .context("compact response")?;
    assert!(compact.get("error").is_none(), "{compact}");
    wait_for_session_notification(&mut notifications, "context/compactionCompleted", session)
        .await?;
    wait_for_parent_turn_completed(&mut notifications, session).await?;
    drop(runtime);

    let check = format!(
        "r = await rlm.history.read('{item_id}')\nassert 'evidence-438' in r['text']\nprint('cold-' + 'verified')"
    );
    let branch_check = format!(
        "assert not (await rlm.history.search('evidence-438', kind='tool_result'))['matches']\ntry:\n    await rlm.history.read('{item_id}')\nexcept RuntimeError:\n    print('branch-' + 'verified')\nelse:\n    raise AssertionError('abandoned evidence is readable')"
    );
    let cold = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::ipython_call("verify-cold", &check),
        ScriptedProvider::completed("done"),
        ScriptedProvider::completed("done"),
        ScriptedProvider::ipython_call("verify-branch", &branch_check),
        ScriptedProvider::completed("done"),
    ]));
    let runtime = build_runtime(home.path(), cold.clone())?;
    let (connection, mut notifications) = initialize_connection(&runtime).await?;
    let resumed = runtime
        .handle_incoming(
            connection,
            json!({"id":81,"method":"session/resume","params":{"sessionId":session}}),
        )
        .await
        .context("resume response")?;
    assert!(resumed.get("error").is_none(), "{resumed}");
    start_turn(&runtime, connection, session, "Read the archived evidence.").await?;
    wait_for_parent_turn_completed(&mut notifications, session).await?;
    let requests = cold.task_requests();
    assert_eq!(requests.len(), 2);
    assert!(
        message_texts(&requests[0])
            .iter()
            .any(|text| text.contains("<compaction_summary>"))
    );
    assert!(
        result_text(&requests[1])
            .last()
            .is_some_and(|s| s.contains("cold-verified"))
    );
    wait_for_session_idle(&runtime, connection, session).await?;
    let items = runtime
        .handle_incoming(
            connection,
            json!({"id":82,"method":"session/items/list","params":{"sessionId":session}}),
        )
        .await
        .context("list items")?;
    let first_checkpoint = items["result"]["data"]
        .as_array()
        .context("transcript rows")?
        .iter()
        .find(|r| r["item"]["type"] == "contextCompaction")
        .context("first checkpoint")?["id"]
        .clone();
    let moved = runtime.handle_incoming(connection, json!({"id":84,"method":"session/tree/navigate","params":{"sessionId":session,"entryId":first_checkpoint}})).await.context("navigate to earlier checkpoint")?;
    assert!(moved.get("error").is_none(), "{moved}");
    start_turn(
        &runtime,
        connection,
        session,
        "Continue from this checkpoint.",
    )
    .await?;
    wait_for_parent_turn_completed(&mut notifications, session).await?;
    let requests = cold.task_requests();
    assert!(
        message_texts(&requests[2])
            .iter()
            .any(|text| text.contains("call:measure"))
    );
    assert!(
        result_text(&requests[2]).is_empty(),
        "rewind reinserted archived tool results"
    );
    wait_for_session_idle(&runtime, connection, session).await?;

    let root_id = items["result"]["data"]
        .as_array()
        .context("items")?
        .iter()
        .find(|r| r["item"]["type"] == "userMessage")
        .context("first user input")?["id"]
        .clone();
    let moved = runtime.handle_incoming(connection, json!({"id":83,"method":"session/tree/navigate","params":{"sessionId":session,"entryId":root_id}})).await.context("navigate")?;
    assert!(moved.get("error").is_none(), "{moved}");
    start_turn(&runtime, connection, session, "Verify the branch boundary.").await?;
    wait_for_parent_turn_completed(&mut notifications, session).await?;
    let requests = cold.task_requests();
    assert!(
        result_text(requests.last().unwrap())
            .last()
            .is_some_and(|s| s.contains("branch-verified"))
    );
    Ok(())
}

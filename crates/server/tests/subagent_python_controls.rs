//! Real CPython cells through the production host bridge and session actors.
#[path = "support/subagent_lifecycle.rs"]
mod support;

use anyhow::{Context, Result};
use pretty_assertions::assert_eq;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use support::*;
use tempfile::TempDir;

#[tokio::test]
async fn python_collect_progress_and_graceful_end_turn() -> Result<()> {
    let home = TempDir::new()?;
    let provider = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::ipython_code_tool_call(
            r#"
from pathlib import Path
worker = await rlm.spawn('Report progress, then finish', name='worker')
import asyncio
results, concurrent = await asyncio.gather(
    rlm.collect([worker, 'worker'], timeout_ms=10000),
    rlm.collect(worker, timeout_ms=10000),
)
assert results == concurrent
assert len(results) == 1, results
result = results[0]
assert result.settled and result.status == 'completed', result
assert result.answer_text == 'child final answer', result
assert result.progress_note == 'testing', result
assert (await rlm.collect(worker))[0] == result
assert (await rlm.list_subagents())[0].progress_note == 'testing'
assert (await rlm.end_turn())['ended']
Path('controls-ok').write_text('complete')
"#,
        ),
        ScriptedProvider::ipython_code_tool_call(
            r#"
assert (await rlm.progress_note('testing')).accepted
again = await rlm.progress_note('too soon')
assert not again.accepted and 0 < again.retry_after_ms <= 10000
"#,
        ),
        ScriptedProvider::completed("child final answer"),
    ]));
    let runtime = build_runtime(home.path(), provider.clone())?;
    let (connection, mut notifications) = initialize_connection(&runtime).await?;
    let parent = start_parent_session(&runtime, connection, home.path()).await?;
    start_turn_with_approval_policy(
        &runtime,
        connection,
        parent,
        "Coordinate children",
        Some("never"),
    )
    .await?;
    let completed =
        wait_for_session_notification(&mut notifications, "turn/completed", parent).await?;
    assert_eq!(
        completed["params"]["turn"]["status"],
        json!("completed"),
        "{completed}"
    );
    assert_eq!(
        std::fs::read_to_string(home.path().join("controls-ok"))?,
        "complete"
    );
    // end_turn prevents another task-script request. Child completion still
    // receives its own host-authored inbox turn without inventing human input.
    assert_eq!(provider.task_stream_calls(), 3);
    Ok(())
}

#[tokio::test]
async fn collect_timeout_does_not_cancel_child_or_block_read_rpcs() -> Result<()> {
    let home = TempDir::new()?;
    let provider = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::ipython_code_tool_call(
            r#"
from pathlib import Path
worker = await rlm.spawn('Keep working', name='worker')
partial = await rlm.collect(worker, timeout_ms=50)
assert len(partial) == 1 and not partial[0].settled, partial
Path('timeout-ok').write_text('still running')
await rlm.collect(worker, timeout_ms=300000)
"#,
        ),
        StreamScript::Pending,
    ]));
    let runtime = build_runtime(home.path(), provider.clone())?;
    let (connection, mut notifications) = initialize_connection(&runtime).await?;
    let parent = start_parent_session(&runtime, connection, home.path()).await?;
    start_turn_with_approval_policy(
        &runtime,
        connection,
        parent,
        "Wait for child",
        Some("never"),
    )
    .await?;
    wait_for_task_stream_calls(&provider, 2).await?;
    // Observable file proves the real kernel returned a partial snapshot.
    tokio::time::timeout(Duration::from_secs(10), async {
        while !home.path().join("timeout-ok").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    for (index, method, params) in [
        (41, "runtime/ping", json!({})),
        (42, "session/read", json!({"sessionId":parent})),
        (43, "session/list", json!({})),
    ] {
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            runtime.handle_incoming(
                connection,
                json!({"id":index, "method":method, "params":params}),
            ),
        )
        .await?
        .context("read response")?;
        assert!(result.get("error").is_none(), "{result}");
    }
    let agents = request_agent_list(&runtime, connection, parent).await?;
    assert_eq!(agents.agents[0].status, "running");
    let interrupt = runtime
        .handle_incoming(
            connection,
            json!({
                "id":44, "method":"session/interrupt", "params":{"scope":{"scope":"session", "sessionId":parent}}
            }),
        )
        .await
        .context("interrupt")?;
    assert!(interrupt.get("error").is_none(), "{interrupt}");
    let completed =
        wait_for_session_notification(&mut notifications, "turn/completed", parent).await?;
    assert_eq!(completed["params"]["turn"]["status"], json!("interrupted"));
    // Explicit session interruption retains the existing cascading child policy.
    request_agent_close(&runtime, connection, parent, "worker").await?;
    Ok(())
}

#[tokio::test]
async fn collect_restores_direct_children_and_answers_after_restart() -> Result<()> {
    let home = TempDir::new()?;
    let first_provider = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::completed("durable answer"),
    ]));
    let first = build_runtime(home.path(), first_provider.clone())?;
    let (connection, mut notifications) = initialize_connection(&first).await?;
    let parent = start_parent_session(&first, connection, home.path()).await?;
    let child = spawn_child_named(
        &first,
        parent,
        "Complete the task",
        Some("none"),
        Some("retained"),
    )
    .await?;
    wait_for_session_notification(&mut notifications, "turn/completed", child.child_session_id)
        .await?;
    wait_for_completion_requests(&first_provider, /*expected*/ 1).await?;
    wait_for_session_idle(&first, connection, parent).await?;
    first.shutdown().await;
    drop(first);
    let provider = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::ipython_code_tool_call(
            r#"
from pathlib import Path
result = (await rlm.collect('retained', timeout_ms=1000))[0]
assert result.settled and result.answer_text == 'durable answer', result
Path('restored-ok').write_text(result.answer_text)
await rlm.end_turn()
"#,
        ),
    ]));
    let restored = build_runtime(home.path(), provider.clone())?;
    restored.load_persisted_sessions().await?;
    let (connection, mut notifications) = initialize_connection(&restored).await?;
    let response = restored
        .handle_incoming(
            connection,
            json!({
                "id":45,"method":"session/resume","params":{"sessionId":parent}
            }),
        )
        .await
        .context("resume parent")?;
    assert!(response.get("error").is_none(), "{response}");
    start_turn_with_approval_policy(
        &restored,
        connection,
        parent,
        "Collect retained answer",
        Some("never"),
    )
    .await?;
    let completed =
        wait_for_session_notification(&mut notifications, "turn/completed", parent).await?;
    assert_eq!(
        completed["params"]["turn"]["status"],
        json!("completed"),
        "{completed}"
    );
    assert_eq!(
        std::fs::read_to_string(home.path().join("restored-ok"))?,
        "durable answer"
    );
    assert_eq!(provider.task_stream_calls(), 1);
    Ok(())
}

#[tokio::test]
async fn python_create_session_is_an_independent_visible_durable_root() -> Result<()> {
    let home = TempDir::new()?;
    let provider = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::ipython_code_tool_call(
            r#"
from pathlib import Path
import json
root = await rlm.create_session('Work independently', name='independent', cwd='.')
assert root.active_session_id == root.session_id
assert root.session_file.is_file()
assert not await rlm.list_subagents()
Path('root-handle.json').write_text(json.dumps({'id': root.session_id, 'journal': str(root.session_file)}))
await rlm.end_turn()
"#,
        ),
        ScriptedProvider::completed_after(Duration::from_millis(100), "independent answer"),
    ]));
    let runtime = build_runtime(home.path(), provider.clone())?;
    let (connection, mut notifications) = initialize_connection(&runtime).await?;
    let parent = start_parent_session(&runtime, connection, home.path()).await?;
    start_turn_with_approval_policy(
        &runtime,
        connection,
        parent,
        "Create an independent root",
        Some("never"),
    )
    .await?;
    let completed =
        wait_for_session_notification(&mut notifications, "turn/completed", parent).await?;
    assert_eq!(
        completed["params"]["turn"]["status"],
        json!("completed"),
        "{completed}"
    );
    let handle: serde_json::Value =
        serde_json::from_slice(&std::fs::read(home.path().join("root-handle.json"))?)?;
    let created_id: devo_protocol::SessionId = handle["id"].as_str().context("root id")?.parse()?;
    assert_ne!(created_id, parent);
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let read = runtime
                .handle_incoming(
                    connection,
                    json!({"id":46, "method":"session/read", "params":{"sessionId":created_id}}),
                )
                .await
                .context("read independent root")?;
            assert!(read.get("error").is_none(), "{read}");
            let session = &read["result"]["session"];
            assert!(session["parent"].is_null(), "{session}");
            assert_eq!(session["title"], json!("independent"));
            assert_eq!(
                session["settings"]["permissionProfile"],
                json!("fullAccess")
            );
            if session["status"] == "idle" {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await??;
    let list = runtime
        .handle_incoming(
            connection,
            json!({"id":47,"method":"session/list","params":{}}),
        )
        .await
        .context("list roots")?;
    assert!(list.to_string().contains(created_id.as_str()), "{list}");
    assert!(
        request_agent_list(&runtime, connection, parent)
            .await?
            .agents
            .is_empty()
    );
    assert_eq!(provider.task_stream_calls(), 2);
    let journal = std::fs::read_to_string(handle["journal"].as_str().context("journal")?)?;
    assert!(journal.contains("independent answer"));
    Ok(())
}

#[tokio::test]
async fn end_turn_keeps_a_running_child_alive() -> Result<()> {
    let home = TempDir::new()?;
    let provider = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::ipython_code_tool_call(
            r#"
from pathlib import Path
worker = await rlm.spawn('Continue after parent returns', name='worker')
partial = await rlm.collect(worker, timeout_ms=10)
assert not partial[0].settled
await rlm.end_turn()
Path('ended-ok').write_text('ended')
"#,
        ),
        StreamScript::Pending,
    ]));
    let runtime = build_runtime(home.path(), provider.clone())?;
    let (connection, mut notifications) = initialize_connection(&runtime).await?;
    let parent = start_parent_session(&runtime, connection, home.path()).await?;
    start_turn_with_approval_policy(
        &runtime,
        connection,
        parent,
        "Yield normally",
        Some("never"),
    )
    .await?;
    let completed =
        wait_for_session_notification(&mut notifications, "turn/completed", parent).await?;
    assert_eq!(completed["params"]["turn"]["status"], json!("completed"));
    assert_eq!(
        std::fs::read_to_string(home.path().join("ended-ok"))?,
        "ended"
    );
    let agents = request_agent_list(&runtime, connection, parent).await?;
    assert_eq!(agents.agents[0].status, "running");
    wait_for_task_stream_calls(&provider, 2).await?;
    assert_eq!(provider.task_stream_calls(), 2);
    request_agent_close(&runtime, connection, parent, "worker").await?;
    Ok(())
}

#[tokio::test]
async fn collect_reports_recoverable_failure_with_its_canonical_error() -> Result<()> {
    let home = TempDir::new()?;
    let provider = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::ipython_code_tool_call(
            r#"
from pathlib import Path
worker = await rlm.spawn('Fail the model request', name='failure')
result = (await rlm.collect(worker, timeout_ms=10000))[0]
assert not result.settled and result.status == 'waiting_for_resume', result
assert result.requires_resume and result.answer_text is None, result
assert 'scripted provider stream script exhausted' in result.error, result
await rlm.delete_subagent(worker.rlm_child_id)
closed = (await rlm.collect(worker))[0]
assert closed.settled and not closed.requires_resume and closed.status == 'failed', closed
assert closed.error == result.error, closed
Path('failure-ok').write_text('reported')
await rlm.end_turn()
"#,
        ),
    ]));
    let runtime = build_runtime(home.path(), provider.clone())?;
    let (connection, mut notifications) = initialize_connection(&runtime).await?;
    let parent = start_parent_session(&runtime, connection, home.path()).await?;
    start_turn_with_approval_policy(
        &runtime,
        connection,
        parent,
        "Collect failure",
        Some("never"),
    )
    .await?;
    let completed =
        wait_for_session_notification(&mut notifications, "turn/completed", parent).await?;
    assert_eq!(completed["params"]["turn"]["status"], json!("completed"));
    assert_eq!(
        std::fs::read_to_string(home.path().join("failure-ok"))?,
        "reported"
    );
    Ok(())
}

#[tokio::test]
async fn collect_recognizes_shutdown_interrupted_child_recovery() -> Result<()> {
    let home = TempDir::new()?;
    let first_provider = Arc::new(ScriptedProvider::pending_agent_tasks());
    let first = build_runtime(home.path(), first_provider.clone())?;
    let (connection, _) = initialize_connection(&first).await?;
    let parent = start_parent_session(&first, connection, home.path()).await?;
    let child = spawn_child_named(
        &first,
        parent,
        "Keep working",
        Some("none"),
        Some("recoverable"),
    )
    .await?;
    wait_for_task_stream_calls(&first_provider, 1).await?;
    first.shutdown().await;
    drop(first);
    let provider = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::ipython_code_tool_call(
            r#"
from pathlib import Path
result = (await rlm.collect('recoverable', timeout_ms=300000))[0]
# An interrupted claimed inbox entry is uncertain, so recovery exposes a
# review barrier instead of replaying its input automatically.
assert result.status == 'waiting_for_review' and result.requires_resume, result
assert not result.settled and result.answer_text is None, result
Path('shutdown-recovery-ok').write_text('no replay')
await rlm.end_turn()
"#,
        ),
    ]));
    let restored = build_runtime(home.path(), provider.clone())?;
    restored.load_persisted_sessions().await?;
    let (connection, mut notifications) = initialize_connection(&restored).await?;
    let response = restored
        .handle_incoming(
            connection,
            json!({
                "id": 81, "method": "session/resume", "params": {"sessionId": parent}
            }),
        )
        .await
        .context("resume parent")?;
    assert!(response.get("error").is_none(), "{response}");
    start_turn_with_approval_policy(
        &restored,
        connection,
        parent,
        "Inspect recovery",
        Some("never"),
    )
    .await?;
    let completed =
        wait_for_session_notification(&mut notifications, "turn/completed", parent).await?;
    let tool_outputs: Vec<_> = provider
        .task_requests()
        .iter()
        .flat_map(|request| {
            request
                .messages
                .iter()
                .flat_map(|message| message.content.iter())
                .filter_map(|content| {
                    if let devo_protocol::RequestContent::ToolResult { content, .. } = content {
                        Some(content.clone())
                    } else {
                        None
                    }
                })
        })
        .collect();
    assert_eq!(
        completed["params"]["turn"]["status"],
        json!("completed"),
        "{completed}; Python results: {tool_outputs:?}"
    );
    assert_eq!(
        std::fs::read_to_string(home.path().join("shutdown-recovery-ok"))?,
        "no replay"
    );
    let queue = restored.handle_incoming(connection, json!({
        "id":82, "method":"session/queue/list", "params":{"sessionId":child.child_session_id}
    })).await.context("read recovered child inbox")?;
    let entries = queue["result"]["entries"]
        .as_array()
        .context("recovered queue entries")?;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["recoveryRequired"], true);
    assert_eq!(
        entries[0]["input"],
        json!([{"type":"text","text":"Keep working"}])
    );
    assert_eq!(provider.task_stream_calls(), 1);
    Ok(())
}

#[tokio::test]
async fn collect_waits_through_a_live_queued_followup() -> Result<()> {
    let home = TempDir::new()?;
    let provider = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::ipython_code_tool_call(
            r#"
from pathlib import Path
worker = await rlm.spawn('Do the first task', name='queued')
result = (await rlm.collect(worker, timeout_ms=10000))[0]
print(result)
assert result.settled and not result.requires_resume, result
assert result.answer_text == 'queued answer', result
Path('queued-ok').write_text('joined both turns')
await rlm.end_turn()
"#,
        ),
        ScriptedProvider::ipython_code_tool_call(
            r#"
import asyncio
from pathlib import Path
while not Path('queue-installed').exists():
    await asyncio.sleep(0.01)
"#,
        ),
        ScriptedProvider::completed("first answer"),
        ScriptedProvider::completed("queued answer"),
    ]));
    let runtime = build_runtime(home.path(), provider.clone())?;
    let (connection, mut notifications) = initialize_connection(&runtime).await?;
    let parent = start_parent_session(&runtime, connection, home.path()).await?;
    start_turn_with_approval_policy(
        &runtime,
        connection,
        parent,
        "Join queued work",
        Some("never"),
    )
    .await?;
    wait_for_task_stream_calls(&provider, 2).await?;
    request_agent_send_message(&runtime, connection, parent, "queued", "Do the follow-up").await?;
    std::fs::write(home.path().join("queue-installed"), "ready")?;
    let completed =
        wait_for_session_notification(&mut notifications, "turn/completed", parent).await?;
    assert_eq!(
        completed["params"]["turn"]["status"],
        json!("completed"),
        "{completed}"
    );
    let outputs = provider
        .task_requests()
        .into_iter()
        .flat_map(|r| r.messages)
        .flat_map(|m| m.content)
        .filter_map(|content| match content {
            devo_protocol::RequestContent::ToolResult { content, .. } => Some(content),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        home.path().join("queued-ok").exists(),
        "Python outputs: {outputs:?}"
    );
    assert_eq!(
        std::fs::read_to_string(home.path().join("queued-ok"))?,
        "joined both turns"
    );
    Ok(())
}

#[tokio::test]
async fn closed_failure_stays_settled_after_restart() -> Result<()> {
    let home = TempDir::new()?;
    let provider = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::ipython_code_tool_call(
            r#"
from pathlib import Path
worker = await rlm.spawn('Fail the model request', name='failure')
result = (await rlm.collect(worker, timeout_ms=10000))[0]
assert not result.settled and result.status == 'waiting_for_resume', result
assert result.requires_resume and result.answer_text is None, result
assert 'scripted provider stream script exhausted' in result.error, result
await rlm.delete_subagent(worker.rlm_child_id)
closed = (await rlm.collect(worker))[0]
assert closed.settled and not closed.requires_resume and closed.status == 'failed', closed
assert closed.error == result.error, closed
Path('failure-ok').write_text('reported')
await rlm.end_turn()
"#,
        ),
    ]));
    let runtime = build_runtime(home.path(), provider.clone())?;
    let (connection, mut notifications) = initialize_connection(&runtime).await?;
    let parent = start_parent_session(&runtime, connection, home.path()).await?;
    start_turn_with_approval_policy(
        &runtime,
        connection,
        parent,
        "Collect failure",
        Some("never"),
    )
    .await?;
    let completed =
        wait_for_session_notification(&mut notifications, "turn/completed", parent).await?;
    assert_eq!(completed["params"]["turn"]["status"], json!("completed"));
    assert_eq!(
        std::fs::read_to_string(home.path().join("failure-ok"))?,
        "reported"
    );
    wait_for_completion_requests(&provider, /*expected*/ 1).await?;
    wait_for_session_idle(&runtime, connection, parent).await?;
    runtime.shutdown().await;
    drop(runtime);
    let provider = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::ipython_code_tool_call(
            r#"
from pathlib import Path
closed = (await rlm.collect('failure'))[0]
import json
Path('after-restart.json').write_text(json.dumps({'status': closed.status, 'settled': closed.settled, 'requires_resume': closed.requires_resume}))
assert closed.settled and not closed.requires_resume, closed
await rlm.end_turn()
"#,
        ),
    ]));
    let restored = build_runtime(home.path(), provider.clone())?;
    restored.load_persisted_sessions().await?;
    let (connection, mut notifications) = initialize_connection(&restored).await?;
    let response = restored
        .handle_incoming(
            connection,
            json!({
                "id":81,"method":"session/resume","params":{"sessionId":parent}
            }),
        )
        .await
        .context("resume parent")?;
    assert!(response.get("error").is_none(), "{response}");
    start_turn_with_approval_policy(
        &restored,
        connection,
        parent,
        "Collect closed failure",
        Some("never"),
    )
    .await?;
    wait_for_session_notification(&mut notifications, "turn/completed", parent).await?;
    let actual: serde_json::Value =
        serde_json::from_slice(&std::fs::read(home.path().join("after-restart.json"))?)?;
    assert_eq!(
        actual,
        json!({"status":"failed", "settled":true, "requires_resume":false})
    );
    Ok(())
}

#[tokio::test]
async fn explicitly_closed_running_child_stays_settled_after_restart() -> Result<()> {
    let home = TempDir::new()?;
    let first_provider = Arc::new(ScriptedProvider::pending_agent_tasks());
    let first = build_runtime(home.path(), first_provider.clone())?;
    let (connection, _) = initialize_connection(&first).await?;
    let parent = start_parent_session(&first, connection, home.path()).await?;
    spawn_child_named(
        &first,
        parent,
        "Keep working",
        Some("none"),
        Some("recoverable"),
    )
    .await?;
    wait_for_task_stream_calls(&first_provider, 1).await?;
    request_agent_close(&first, connection, parent, "recoverable").await?;
    wait_for_completion_requests(&first_provider, /*expected*/ 1).await?;
    wait_for_session_idle(&first, connection, parent).await?;
    first.shutdown().await;
    drop(first);
    let provider = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::ipython_code_tool_call(
            r#"
from pathlib import Path
result = (await rlm.collect('recoverable', timeout_ms=300000))[0]
assert result.status == 'interrupted' and not result.requires_resume, result
assert result.settled and result.answer_text is None, result
Path('shutdown-recovery-ok').write_text('no replay')
await rlm.end_turn()
"#,
        ),
    ]));
    let restored = build_runtime(home.path(), provider.clone())?;
    restored.load_persisted_sessions().await?;
    let (connection, mut notifications) = initialize_connection(&restored).await?;
    let response = restored
        .handle_incoming(
            connection,
            json!({
                "id": 81, "method": "session/resume", "params": {"sessionId": parent}
            }),
        )
        .await
        .context("resume parent")?;
    assert!(response.get("error").is_none(), "{response}");
    start_turn_with_approval_policy(
        &restored,
        connection,
        parent,
        "Inspect recovery",
        Some("never"),
    )
    .await?;
    let completed =
        wait_for_session_notification(&mut notifications, "turn/completed", parent).await?;
    assert_eq!(
        completed["params"]["turn"]["status"],
        json!("completed"),
        "{completed}"
    );
    assert_eq!(
        std::fs::read_to_string(home.path().join("shutdown-recovery-ok"))?,
        "no replay"
    );
    assert_eq!(provider.task_stream_calls(), 1);
    Ok(())
}

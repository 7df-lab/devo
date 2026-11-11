//! Factory checkpoints and guards through real CPython, host RPC and session actors.
#[path = "support/subagent_lifecycle.rs"]
mod support;

use anyhow::{Context, Result};
use pretty_assertions::assert_eq;
use serde_json::json;
use std::sync::Arc;
use support::*;

#[tokio::test]
async fn durable_guarded_diamond_runs_only_selected_branch_and_joins_output() -> Result<()> {
    let home = tempfile::TempDir::new()?;
    let provider = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::ipython_code_tool_call(
            r#"
from pathlib import Path
spec = {'version': 2, 'nodes': [
    {'id': 'review', 'prompt': 'Return an approval verdict as JSON.'},
    {'id': 'accept', 'prompt': 'Handle the approved branch.', 'needs': ['review'],
     'when': {'node': 'review', 'path': ['approved'], 'equals': True}},
    {'id': 'reject', 'prompt': 'Handle the rejected branch.', 'needs': ['review'],
     'when': {'node': 'review', 'path': ['approved'], 'equals': False}},
    {'id': 'join', 'prompt': 'Summarize the selected result.', 'needs': ['accept', 'reject'], 'join': 'any'}
]}
run = await rlm.factory.start(spec, enabled=True, durable=True)
result = await rlm.factory.wait(run['id'], timeout=10, poll_interval=0.01)
assert result['status'] == 'completed', result
assert result['nodes']['reject']['status'] == 'skipped', result
assert result['nodes']['join']['output'] == 'joined result', result
assert len(await rlm.list_subagents()) == 3
saved = await rlm.factory.saved()
assert len(saved) == 1 and saved[0]['status'] == 'completed', saved
assert await rlm.factory.stop(run['id']) == result
Path('factory-guard-ok').write_text(run['id'])
await rlm.end_turn()
"#,
        ),
        ScriptedProvider::completed(r#"{"approved": true}"#),
        ScriptedProvider::completed("accepted result"),
        ScriptedProvider::completed("joined result"),
    ]));
    let runtime = build_runtime(home.path(), provider.clone())?;
    let (connection, mut notifications) = initialize_connection(&runtime).await?;
    let parent = start_parent_session(&runtime, connection, home.path()).await?;
    start_turn_with_approval_policy(
        &runtime,
        connection,
        parent,
        "Run a guarded workflow",
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
        std::fs::read_to_string(home.path().join("factory-guard-ok"))?.len(),
        32
    );
    let requests = provider.task_requests();
    assert_eq!(requests.len(), 4);
    let join = message_texts(&requests[3]).join("\n");
    assert!(join.contains(r#""accept": "accepted result""#), "{join}");
    assert!(!join.contains(r#""reject":"#), "{join}");
    Ok(())
}

#[tokio::test]
async fn durable_factory_recovers_existing_child_after_runtime_restart_without_respawn()
-> Result<()> {
    let home = tempfile::TempDir::new()?;
    let first_provider = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::ipython_code_tool_call(
            r#"
from pathlib import Path
run = await rlm.factory.start({'version': 1, 'nodes': [
    {'id': 'research', 'prompt': 'Research once.'},
    {'id': 'join', 'prompt': 'Summarize the retained research.', 'needs': ['research']}
]}, enabled=True, durable=True)
state = await rlm.factory.advance(run['id'])
child = state['nodes']['research']['child_id']
assert (await rlm.collect(child, timeout_ms=10000))[0].answer_text == 'retained research'
# The durable snapshot still says running. Recovery must reconcile the retained
# child, not trust stale status or dispatch the research a second time.
Path('factory-resume-id').write_text(run['id'])
await rlm.end_turn()
"#,
        ),
        ScriptedProvider::completed("retained research"),
    ]));
    let first = build_runtime(home.path(), first_provider.clone())?;
    let (connection, mut notifications) = initialize_connection(&first).await?;
    let parent = start_parent_session(&first, connection, home.path()).await?;
    start_turn_with_approval_policy(
        &first,
        connection,
        parent,
        "Start durable work",
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
    assert_eq!(first_provider.task_stream_calls(), 2);
    wait_for_completion_requests(&first_provider, /*expected*/ 1).await?;
    wait_for_session_idle(&first, connection, parent).await?;
    first.shutdown().await;
    drop(first);

    let provider = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::ipython_code_tool_call(
            r#"
from pathlib import Path
run_id = Path('factory-resume-id').read_text()
saved = await rlm.factory.saved()
assert len(saved) == 1 and saved[0]['run_id'] == run_id, saved
restored = await rlm.factory.restore(run_id, enabled=True)
assert restored['durable'] and restored['status'] == 'running', restored
result = await rlm.factory.wait(run_id, timeout=10, poll_interval=0.01)
assert result['status'] == 'completed', result
assert result['nodes']['research']['output'] == 'retained research', result
assert result['nodes']['join']['output'] == 'resumed summary', result
assert len(await rlm.list_subagents()) == 2
Path('factory-resumed-ok').write_text('no duplicate research')
await rlm.end_turn()
"#,
        ),
        ScriptedProvider::completed("resumed summary"),
    ]));
    let restored = build_runtime(home.path(), provider.clone())?;
    restored.load_persisted_sessions().await?;
    let (connection, mut notifications) = initialize_connection(&restored).await?;
    let response = restored
        .handle_incoming(
            connection,
            json!({
                "id":45, "method":"session/resume", "params":{"sessionId":parent}
            }),
        )
        .await
        .context("resume parent")?;
    assert!(response.get("error").is_none(), "{response}");
    start_turn_with_approval_policy(
        &restored,
        connection,
        parent,
        "Restore durable work",
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
        std::fs::read_to_string(home.path().join("factory-resumed-ok"))?,
        "no duplicate research"
    );
    assert_eq!(provider.task_stream_calls(), 2);
    let requests = provider.task_requests();
    let join = message_texts(&requests[1]).join("\n");
    assert!(
        join.contains(r#""research": "retained research""#),
        "{join}"
    );
    Ok(())
}

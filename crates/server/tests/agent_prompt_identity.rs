use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use devo_core::tools::create_default_tool_registry;
use devo_protocol::SessionId;
use devo_server::ServerRuntime;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use tempfile::TempDir;

#[path = "support/subagent_lifecycle.rs"]
mod support;

use support::ScriptedProvider;
use support::initialize_connection;
use support::spawn_child_named;
use support::start_parent_session;
use support::start_turn;
use support::wait_for_parent_turn_completed;

fn build_runtime(data_root: &Path, provider: Arc<ScriptedProvider>) -> Arc<ServerRuntime> {
    devo_server::test_support::TestRuntime::new(provider)
        .with_test_model()
        .registry(Arc::new(create_default_tool_registry()))
        .runtime(data_root)
}

async fn rpc(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    method: &str,
    session_id: SessionId,
) -> Result<Value> {
    let response = runtime
        .handle_incoming(
            connection_id,
            json!({ "id": 100, "method": method, "params": { "sessionId": session_id } }),
        )
        .await
        .with_context(|| format!("missing {method} response"))?;
    assert!(response.get("error").is_none(), "{method}: {response}");
    Ok(response["result"].clone())
}

async fn read_prompt(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: SessionId,
) -> Result<String> {
    let result = rpc(
        runtime,
        connection_id,
        "session/systemPrompt/read",
        session_id,
    )
    .await?;
    Ok(result["prompt"]
        .as_str()
        .context("system prompt")?
        .to_owned())
}

// Compare the complete identity-bearing portion without coupling these tests to
// unrelated workspace, web-tool, or catalog instructions.
fn assert_identity(prompt: &str, depth: u32, parent: Option<&str>) {
    let actual = prompt
        .lines()
        .filter(|line| {
            line.starts_with("Recursive agent depth:")
                || line.starts_with("You are a child agent spawned by ")
        })
        .collect::<Vec<_>>();
    let mut expected = vec![format!("Recursive agent depth: {depth}")];
    if let Some(parent) = parent {
        expected.push(format!(
            "You are a child agent spawned by {parent}. Your delegated task is provided in the session input."
        ));
    }
    assert_eq!(actual, expected);
    assert_eq!(
        (
            prompt.contains("As the user-facing root agent"),
            prompt.contains("You run in your own RLM kernel namespace"),
            prompt.contains("Auto-refine is disabled for child sessions"),
        ),
        (parent.is_none(), parent.is_some(), parent.is_some())
    );
}

#[tokio::test]
async fn root_and_ordinary_fork_keep_root_identity_before_and_after_capture() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(ScriptedProvider::new([
        ScriptedProvider::completed("root done"),
        ScriptedProvider::completed("fork done"),
    ]));
    let runtime = build_runtime(data_root.path(), Arc::clone(&provider));
    let (connection_id, mut notifications) = initialize_connection(&runtime).await?;
    let root = start_parent_session(&runtime, connection_id, data_root.path()).await?;

    assert_identity(
        &read_prompt(&runtime, connection_id, root).await?,
        /*depth*/ 0,
        /*parent*/ None,
    );
    start_turn(&runtime, connection_id, root, "root task").await?;
    wait_for_parent_turn_completed(&mut notifications, root).await?;
    assert_identity(
        &read_prompt(&runtime, connection_id, root).await?,
        /*depth*/ 0,
        /*parent*/ None,
    );

    let fork = rpc(&runtime, connection_id, "session/fork", root).await?;
    let fork: SessionId = serde_json::from_value(fork["session"]["id"].clone())?;
    assert_identity(
        &read_prompt(&runtime, connection_id, fork).await?,
        /*depth*/ 0,
        /*parent*/ None,
    );
    start_turn(&runtime, connection_id, fork, "fork task").await?;
    wait_for_parent_turn_completed(&mut notifications, fork).await?;
    assert_identity(
        &read_prompt(&runtime, connection_id, fork).await?,
        /*depth*/ 0,
        /*parent*/ None,
    );

    let requests = provider.requests();
    assert_eq!(requests.len(), 2);
    for request in requests {
        assert_identity(
            request.system.as_deref().context("provider system")?,
            /*depth*/ 0,
            /*parent*/ None,
        );
    }
    runtime.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn slash_named_descendants_keep_identity_with_inherited_context_and_resume() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(ScriptedProvider::new([
        ScriptedProvider::completed("child done"),
        ScriptedProvider::completed("grandchild done"),
    ]));
    let runtime = build_runtime(data_root.path(), Arc::clone(&provider));
    let (connection_id, mut notifications) = initialize_connection(&runtime).await?;
    let root = start_parent_session(&runtime, connection_id, data_root.path()).await?;
    // Slashes are valid in nicknames, so path components cannot determine depth.
    let child = spawn_child_named(
        &runtime,
        root,
        "child task",
        Some("none"),
        Some("review/api"),
    )
    .await?;
    assert_eq!(child.agent_path, "root/review/api");
    wait_for_parent_turn_completed(&mut notifications, child.child_session_id).await?;
    // Root subscriptions cover direct children only. Resume subscribes to the
    // child's own descendants before the grandchild starts.
    rpc(
        &runtime,
        connection_id,
        "session/resume",
        child.child_session_id,
    )
    .await?;
    let grandchild = spawn_child_named(
        &runtime,
        child.child_session_id,
        "grandchild task",
        Some("all"),
        Some("grandchild"),
    )
    .await?;
    wait_for_parent_turn_completed(&mut notifications, grandchild.child_session_id).await?;
    assert_eq!(grandchild.agent_path, "root/review/api/grandchild");

    let descendants = [
        (child.child_session_id, 1, "root"),
        (grandchild.child_session_id, 2, "root/review/api"),
    ];
    let requests = provider.requests();
    assert_eq!(requests.len(), descendants.len());
    for ((session_id, depth, parent), request) in descendants.iter().zip(&requests) {
        assert_identity(
            request.system.as_deref().context("provider system")?,
            *depth,
            Some(parent),
        );
        assert_identity(
            &read_prompt(&runtime, connection_id, *session_id).await?,
            *depth,
            Some(parent),
        );
    }
    runtime.shutdown().await;

    let resumed_provider = Arc::new(ScriptedProvider::new([
        ScriptedProvider::completed("child resumed"),
        ScriptedProvider::completed("grandchild resumed"),
    ]));
    let rebuilt = build_runtime(data_root.path(), Arc::clone(&resumed_provider));
    rebuilt.refresh_session_index()?;
    let (connection_id, mut notifications) = initialize_connection(&rebuilt).await?;
    for (session_id, depth, parent) in descendants {
        rpc(&rebuilt, connection_id, "session/resume", session_id).await?;
        // A newly restored actor has no captured SessionContext yet.
        assert_identity(
            &read_prompt(&rebuilt, connection_id, session_id).await?,
            depth,
            Some(parent),
        );
        start_turn(&rebuilt, connection_id, session_id, "continue task").await?;
        wait_for_parent_turn_completed(&mut notifications, session_id).await?;
        assert_identity(
            &read_prompt(&rebuilt, connection_id, session_id).await?,
            depth,
            Some(parent),
        );
    }
    let requests = resumed_provider.requests();
    assert_eq!(requests.len(), descendants.len());
    for ((_, depth, parent), request) in descendants.iter().zip(&requests) {
        assert_identity(
            request
                .system
                .as_deref()
                .context("resumed provider system")?,
            *depth,
            Some(parent),
        );
    }
    rebuilt.shutdown().await;
    Ok(())
}

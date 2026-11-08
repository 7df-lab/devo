//! Real Python kernel -> host -> independently routed child model regressions.
#[path = "support/subagent_lifecycle.rs"]
mod support;

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use devo_core::PresetModelCatalog;
use devo_core::tools::{AgentToolCoordinator, create_default_tool_registry};
use devo_protocol::{
    ModelProfileKey, ModelRequest, ProviderWireApi, ReasoningEffort, SessionId, SpawnAgentParams,
};
use devo_provider::{MultiProviderRouter, ProviderRoute};
use devo_server::{ServerRuntime, test_support::TestRuntime};
use pretty_assertions::assert_eq;
use serde_json::json;
use support::*;
use tempfile::TempDir;

const PARENT: &str = "parent/vendor/main";
const CHILD: &str = "alternate/vendor/child";

fn write_config(home: &Path) -> Result<()> {
    std::fs::write(
        home.join("providers.json"),
        serde_json::to_vec_pretty(&json!({
            "model": PARENT,
            "provider": {
                "parent": {
                    "name": "Parent", "wire_api": "openai_chat_completions",
                    "base_url": "https://parent.invalid/v1", "enabled": true,
                    "models": {"vendor/main": {
                        "name": "Parent model", "request_model": "wire-parent",
                        "default_reasoning_selection": "off",
                        "reasoning_capability": {"levels": ["off", "high"]}
                    }}
                },
                "alternate": {
                    "name": "Alternate", "wire_api": "openai_chat_completions",
                    "base_url": "https://alternate.invalid/v1", "enabled": true,
                    "models": {"vendor/child": {
                        "name": "Child model", "request_model": "wire-child",
                        "reasoning_capability": {"levels": ["off", "high"]}
                    }, "disabled": {"name": "Disabled model", "enabled": false}}
                },
                "unrouted": {
                    "name": "Missing adapter", "wire_api": "openai_chat_completions",
                    "base_url": "https://unrouted.invalid/v1", "enabled": true,
                    "models": {"missing": {"name": "Unrouted model"}}
                }
            }
        }))?,
    )?;
    Ok(())
}

fn runtime(
    home: &Path,
    parent: Arc<ScriptedProvider>,
    child: Arc<ScriptedProvider>,
) -> Arc<ServerRuntime> {
    let mut router = MultiProviderRouter::new(parent.clone());
    router.insert_route(
        ProviderRoute::connection("parent", ProviderWireApi::OpenAIChatCompletions),
        parent.clone(),
    );
    router.insert_route(
        ProviderRoute::connection("alternate", ProviderWireApi::OpenAIChatCompletions),
        child,
    );
    TestRuntime::new(parent)
        .router(Arc::new(router))
        .default_model(PARENT)
        .catalog(Arc::new(PresetModelCatalog::new(
            [PARENT, CHILD]
                .into_iter()
                .map(|slug| devo_protocol::Model {
                    slug: slug.to_string(),
                    display_name: slug.to_string(),
                    reasoning_capability: devo_protocol::ReasoningCapability::Levels(vec![
                        devo_protocol::ReasoningLevelChoice::Off,
                        devo_protocol::ReasoningLevelChoice::Effort(ReasoningEffort::High),
                    ]),
                    default_reasoning_selection: Some("off".to_string()),
                    ..devo_protocol::Model::default()
                })
                .collect(),
        )))
        .registry(Arc::new(create_default_tool_registry()))
        .db_file("child-model-selection.db")
        .runtime(home)
}

async fn start_parent_session(
    runtime: &Arc<ServerRuntime>,
    connection: u64,
    cwd: &Path,
) -> Result<SessionId> {
    let session = support::start_parent_session(runtime, connection, cwd).await?;
    let read = runtime
        .handle_incoming(
            connection,
            json!({
                "id": 50, "method": "session/read", "params": {"sessionId": session}
            }),
        )
        .await
        .context("read parent model selection")?;
    assert_eq!(
        read["result"]["session"]["model"]["model"],
        json!(PARENT),
        "fixture must select its configured parent model: {read}"
    );
    Ok(session)
}

fn bindings(requests: &[ModelRequest]) -> Vec<(ModelProfileKey, String, Option<ReasoningEffort>)> {
    requests
        .iter()
        .map(|request| {
            (
                request.model_slug.clone(),
                request.model.clone(),
                request.reasoning_effort,
            )
        })
        .collect()
}

fn assert_python_stdout(requests: &[ModelRequest], expected: &str) -> Result<()> {
    let outputs = requests
        .iter()
        .flat_map(|request| &request.messages)
        .flat_map(|message| &message.content)
        .filter_map(|content| match content {
            devo_protocol::RequestContent::ToolResult {
                content, is_error, ..
            } => Some((content, is_error)),
            _ => None,
        })
        .map(|(content, is_error)| {
            let output: serde_json::Value = serde_json::from_str(content)?;
            Ok((
                is_error.unwrap_or(false),
                output["details"]["stdout"].clone(),
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    assert!(
        outputs.contains(&(false, json!(expected))),
        "missing Python success output: {outputs:?}"
    );
    assert!(
        outputs.iter().all(|(error, _)| !error),
        "Python tool returned an error: {outputs:?}"
    );
    Ok(())
}

async fn wait_completed(
    runtime: &Arc<ServerRuntime>,
    connection: u64,
    parent: SessionId,
    target: &str,
) -> Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(/*secs*/ 15), async {
        loop {
            let result = request_agent_wait_with(
                runtime,
                connection,
                parent,
                Some(target),
                /*after_sequence*/ None,
                /*timeout_secs*/ 10,
            )
            .await?;
            for event in result.events {
                match event.status.as_deref() {
                    Some("completed") => return Ok(()),
                    Some("failed" | "interrupted" | "closed") => {
                        anyhow::bail!("child {target} terminated: {event:?}")
                    }
                    _ => {}
                }
            }
        }
    })
    .await
    .context("waiting for completed child status")?
}

async fn spawn_selected(
    runtime: &Arc<ServerRuntime>,
    parent: SessionId,
    model: &str,
    thinking: &str,
) -> Result<devo_protocol::SpawnAgentResult> {
    let params: SpawnAgentParams = serde_json::from_value(json!({
        "session_id": parent, "message": "Run the isolated child task", "fork_turns": "none",
        "model": model, "thinking": thinking
    }))?;
    Ok(Arc::clone(runtime).spawn_agent(params).await?)
}

#[tokio::test]
async fn python_spawn_routes_child_sibling_grandchild_and_followup_independently() -> Result<()> {
    let home = TempDir::new()?;
    write_config(home.path())?;
    let parent = Arc::new(ScriptedProvider::new([
        ScriptedProvider::ipython_code_tool_call(
            r#"
available = await rlm.find_models()
assert all(not model.selector.startswith('unrouted/') for model in available)
models = await rlm.find_models('alternate')
assert len(models) == 1
assert models[0].selector == 'alternate/vendor/child'
assert 'high' in models[0].thinking_levels
child = await rlm.spawn('Use your chosen model and spawn a grandchild', name='selected', model=models[0].selector, thinking='high')
assert child.model == models[0].selector
sibling = await rlm.spawn('Inherit the parent model', name='inherited')
assert sibling.model == 'parent/vendor/main'
print('SELECTION_AND_DISCOVERY_OK')
"#,
        ),
        ScriptedProvider::completed("parent or sibling complete"),
        ScriptedProvider::completed("parent or sibling complete"),
    ]));
    let child = Arc::new(ScriptedProvider::new([
        ScriptedProvider::ipython_code_tool_call(
            r#"
grandchild = await rlm.spawn('Inherit the immediate parent model and thinking', name='grandchild')
assert grandchild.model == 'alternate/vendor/child'
print('GRANDCHILD_INHERITANCE_OK')
"#,
        ),
        ScriptedProvider::completed("child or grandchild complete"),
        ScriptedProvider::completed("child or grandchild complete"),
        ScriptedProvider::completed("followup complete"),
    ]));
    let runtime = runtime(home.path(), parent.clone(), child.clone());
    let (connection, mut notifications) = initialize_connection(&runtime).await?;
    let root = start_parent_session(&runtime, connection, home.path()).await?;
    start_turn_with_approval_policy(
        &runtime,
        connection,
        root,
        "Discover and select child models",
        Some("never"),
    )
    .await?;
    let parent_completed =
        wait_for_session_notification(&mut notifications, "turn/completed", root).await?;
    assert_eq!(
        parent_completed["params"]["turn"]["status"],
        json!("completed"),
        "parent turn failed: {parent_completed}; requests: {:?}",
        bindings(&parent.requests())
    );
    wait_for_stream_calls(&parent, 3).await.with_context(|| {
        format!(
            "parent requests: {:?}; child requests: {:?}",
            bindings(&parent.requests()),
            bindings(&child.requests())
        )
    })?;
    wait_for_stream_calls(&child, 3).await?;
    let agents = request_agent_list(&runtime, connection, root).await?;
    let selected = agents
        .agents
        .iter()
        .find(|agent| agent.agent_nickname == "selected")
        .context("selected child")?;
    let selected_id = selected.session_id;
    wait_completed(&runtime, connection, root, "selected").await?;
    request_agent_send_message(
        &runtime,
        connection,
        root,
        "selected",
        "Follow up using the same model",
    )
    .await?;
    wait_for_stream_calls(&child, 4).await?;
    wait_completed(&runtime, connection, root, "selected").await?;
    assert_eq!(
        bindings(&parent.requests()),
        vec![
            (
                ModelProfileKey::CatalogSlug(PARENT.to_string()),
                "wire-parent".to_string(),
                None
            );
            3
        ]
    );
    assert_eq!(
        bindings(&child.requests()),
        vec![
            (
                ModelProfileKey::CatalogSlug(CHILD.to_string()),
                "wire-child".to_string(),
                Some(ReasoningEffort::High)
            );
            4
        ]
    );
    let descendants = request_agent_list(&runtime, connection, selected_id).await?;
    assert!(
        descendants
            .agents
            .iter()
            .any(|agent| agent.agent_nickname == "grandchild")
    );
    assert_python_stdout(&parent.requests(), "SELECTION_AND_DISCOVERY_OK\n")?;
    assert_python_stdout(&child.requests(), "GRANDCHILD_INHERITANCE_OK\n")?;
    Ok(())
}

#[tokio::test]
async fn child_binding_and_thinking_survive_runtime_restart() -> Result<()> {
    let home = TempDir::new()?;
    write_config(home.path())?;
    let parent = Arc::new(ScriptedProvider::new([]));
    let child = Arc::new(ScriptedProvider::new([ScriptedProvider::completed(
        "initial child",
    )]));
    let first = runtime(home.path(), parent.clone(), child.clone());
    let (connection, _notifications) = initialize_connection(&first).await?;
    let root = start_parent_session(&first, connection, home.path()).await?;
    let spawned = spawn_selected(&first, root, CHILD, "high").await?;
    wait_completed(&first, connection, root, &spawned.agent_path).await?;
    assert_eq!(
        bindings(&child.requests()),
        vec![(
            ModelProfileKey::CatalogSlug(CHILD.to_string()),
            "wire-child".to_string(),
            Some(ReasoningEffort::High)
        )]
    );
    drop(first);

    let restored_child = Arc::new(ScriptedProvider::new([ScriptedProvider::completed(
        "restored child",
    )]));
    let restored = runtime(home.path(), parent.clone(), restored_child.clone());
    restored.load_persisted_sessions().await?;
    let (connection, mut notifications) = initialize_connection(&restored).await?;
    let response = restored.handle_incoming(connection, json!({
        "id": 20, "method": "session/resume", "params": {"sessionId": spawned.child_session_id}
    })).await.context("resume child")?;
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(
        response["result"]["session"]["model"]["model"],
        json!(CHILD)
    );
    start_turn(
        &restored,
        connection,
        spawned.child_session_id,
        "Continue after restart",
    )
    .await?;
    wait_for_parent_turn_completed(&mut notifications, spawned.child_session_id).await?;
    assert_eq!(
        bindings(&restored_child.requests()),
        vec![(
            ModelProfileKey::CatalogSlug(CHILD.to_string()),
            "wire-child".to_string(),
            Some(ReasoningEffort::High)
        )]
    );
    assert_eq!(parent.requests().len(), 0);
    Ok(())
}

#[tokio::test]
async fn invalid_child_selection_is_rejected_before_agent_creation_or_provider_call() -> Result<()>
{
    let home = TempDir::new()?;
    write_config(home.path())?;
    let parent = Arc::new(ScriptedProvider::new([]));
    let child = Arc::new(ScriptedProvider::new([]));
    let runtime = runtime(home.path(), parent.clone(), child.clone());
    let (connection, _notifications) = initialize_connection(&runtime).await?;
    let root = start_parent_session(&runtime, connection, home.path()).await?;
    for (model, thinking) in [
        ("child", "high"),
        ("alternate/no-such-model", "high"),
        ("alternate/disabled", "off"),
        (CHILD, "impossible"),
        ("unrouted/missing", "off"),
    ] {
        let result = spawn_selected(&runtime, root, model, thinking).await;
        assert!(
            result.is_err(),
            "unexpected successful selection: {model} / {thinking}"
        );
        assert_eq!(
            request_agent_list(&runtime, connection, root)
                .await?
                .agents
                .len(),
            0
        );
    }
    assert_eq!((parent.requests().len(), child.requests().len()), (0, 0));
    Ok(())
}

#[tokio::test]
async fn restored_child_with_disconnected_route_never_falls_back_to_parent() -> Result<()> {
    let home = TempDir::new()?;
    write_config(home.path())?;
    let parent = Arc::new(ScriptedProvider::new([]));
    let child = Arc::new(ScriptedProvider::new([ScriptedProvider::completed(
        "initial child",
    )]));
    let first = runtime(home.path(), parent.clone(), child.clone());
    let (connection, _notifications) = initialize_connection(&first).await?;
    let root = start_parent_session(&first, connection, home.path()).await?;
    let spawned = spawn_selected(&first, root, CHILD, "high").await?;
    wait_completed(&first, connection, root, &spawned.agent_path).await?;
    drop(first);

    let mut router = MultiProviderRouter::new(parent.clone());
    router.insert_route(
        ProviderRoute::connection("parent", ProviderWireApi::OpenAIChatCompletions),
        parent.clone(),
    );
    let restored = TestRuntime::new(parent.clone())
        .router(Arc::new(router))
        .default_model(PARENT)
        .db_file("child-model-selection.db")
        .runtime(home.path());
    restored.load_persisted_sessions().await?;
    let (connection, _notifications) = initialize_connection(&restored).await?;
    let response = restored.handle_incoming(connection, json!({
        "id": 30, "method": "session/resume", "params": {"sessionId": spawned.child_session_id}
    })).await.context("read disconnected child")?;
    assert!(
        response.get("error").is_none(),
        "read-only resume must remain available: {response}"
    );
    let response = restored
        .handle_incoming(
            connection,
            json!({
                "id": 31, "method": "turn/start", "params": {
                    "sessionId": spawned.child_session_id,
                    "input": [{"type": "text", "text": "Continue with the persisted model"}],
                    "idempotencyKey": "disconnected-child"
                }
            }),
        )
        .await
        .context("start disconnected child")?;
    assert!(
        response["error"]
            .to_string()
            .contains("route is unavailable"),
        "missing child route must fail explicitly: {response}"
    );
    assert_eq!((parent.requests().len(), child.requests().len()), (0, 1));
    Ok(())
}

#[tokio::test]
async fn concurrent_sibling_admissions_keep_distinct_model_bindings() -> Result<()> {
    let home = TempDir::new()?;
    write_config(home.path())?;
    let parent = Arc::new(ScriptedProvider::new([ScriptedProvider::completed(
        "inherited sibling",
    )]));
    let child = Arc::new(ScriptedProvider::new([ScriptedProvider::completed(
        "selected sibling",
    )]));
    let runtime = runtime(home.path(), parent.clone(), child.clone());
    let (connection, _notifications) = initialize_connection(&runtime).await?;
    let root = start_parent_session(&runtime, connection, home.path()).await?;
    let (first, second) = tokio::join!(
        spawn_selected(&runtime, root, PARENT, "off"),
        spawn_selected(&runtime, root, CHILD, "high"),
    );
    let (first, second) = (first?, second?);
    wait_completed(&runtime, connection, root, &first.agent_path).await?;
    wait_completed(&runtime, connection, root, &second.agent_path).await?;
    assert_ne!(first.child_session_id, second.child_session_id);
    assert_eq!(
        request_agent_list(&runtime, connection, root)
            .await?
            .agents
            .len(),
        2
    );
    assert_eq!(
        bindings(&parent.requests()),
        vec![(
            ModelProfileKey::CatalogSlug(PARENT.to_string()),
            "wire-parent".to_string(),
            None
        )]
    );
    assert_eq!(
        bindings(&child.requests()),
        vec![(
            ModelProfileKey::CatalogSlug(CHILD.to_string()),
            "wire-child".to_string(),
            Some(ReasoningEffort::High)
        )]
    );
    Ok(())
}

#[tokio::test]
async fn invalid_selection_does_not_poison_later_spawn_during_active_parent_turn() -> Result<()> {
    let home = TempDir::new()?;
    write_config(home.path())?;
    let parent = Arc::new(ScriptedProvider::pending());
    let child = Arc::new(ScriptedProvider::new([ScriptedProvider::completed(
        "valid child",
    )]));
    let runtime = runtime(home.path(), parent.clone(), child.clone());
    let (connection, mut notifications) = initialize_connection(&runtime).await?;
    let root = start_parent_session(&runtime, connection, home.path()).await?;
    start_turn(&runtime, connection, root, "Keep the parent turn active").await?;
    wait_for_stream_calls(&parent, 1).await.with_context(|| {
        format!(
            "parent requests: {:?}; child requests: {:?}",
            bindings(&parent.requests()),
            bindings(&child.requests())
        )
    })?;
    assert!(
        spawn_selected(&runtime, root, "alternate/no-such-model", "high")
            .await
            .is_err()
    );
    let selected = spawn_selected(&runtime, root, CHILD, "high").await?;
    wait_completed(&runtime, connection, root, &selected.agent_path).await?;
    assert_eq!(
        request_agent_list(&runtime, connection, root)
            .await?
            .agents
            .len(),
        1
    );
    assert_eq!(
        bindings(&child.requests()),
        vec![(
            ModelProfileKey::CatalogSlug(CHILD.to_string()),
            "wire-child".to_string(),
            Some(ReasoningEffort::High)
        )]
    );
    assert_eq!(parent.requests().len(), 1);
    let interrupted = runtime
        .handle_incoming(
            connection,
            json!({
                "id": 40, "method": "session/interrupt", "params": {"scope": {"scope": "session", "sessionId": root}}
            }),
        )
        .await
        .context("interrupt pending parent")?;
    assert!(interrupted.get("error").is_none(), "{interrupted}");
    wait_for_parent_turn_completed(&mut notifications, root).await?;
    Ok(())
}

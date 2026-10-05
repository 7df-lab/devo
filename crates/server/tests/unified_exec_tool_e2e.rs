use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::time::timeout;

#[path = "support/subagent_lifecycle.rs"]
mod support;

use support::{ScriptedProvider, build_runtime, initialize_connection, start_parent_session};

/// Keep the RLM/discrete model surface in place while exercising the native
/// task/process RPCs directly. Shell process management is a client API, not a
/// model-facing tool in this surface.
fn write_discrete_surface_config(data_root: &Path) -> Result<()> {
    std::fs::write(
        data_root.join("config.toml"),
        "[tools]\nexecution_surface = \"discrete\"\n",
    )?;
    Ok(())
}

async fn setup_runtime() -> Result<(
    TempDir,
    Arc<devo_server::ServerRuntime>,
    u64,
    devo_protocol::SessionId,
)> {
    let data_root = TempDir::new()?;
    write_discrete_surface_config(data_root.path())?;
    let provider = Arc::new(ScriptedProvider::pending());
    let runtime = build_runtime(data_root.path(), provider as _)?;
    let (connection_id, _) = initialize_connection(&runtime).await?;
    let session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;
    Ok((data_root, runtime, connection_id, session_id))
}

async fn native_rpc(
    runtime: &Arc<devo_server::ServerRuntime>,
    connection_id: u64,
    method: &str,
    params: Value,
) -> Result<Value> {
    let response = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": uuid::Uuid::new_v4().to_string(),
                "method": method,
                "params": params,
            }),
        )
        .await
        .with_context(|| format!("{method} response"))?;
    if let Some(error) = response.get("error") {
        bail!("{method} failed: {error}");
    }
    response
        .get("result")
        .cloned()
        .with_context(|| format!("{method} result"))
}

async fn start_process(
    runtime: &Arc<devo_server::ServerRuntime>,
    connection_id: u64,
    session_id: devo_protocol::SessionId,
    cwd: &Path,
    command: &str,
) -> Result<String> {
    let result = native_rpc(
        runtime,
        connection_id,
        "task/start",
        json!({
            "kind": "process",
            "sessionId": session_id,
            "command": command,
            "cwd": cwd,
            "idempotencyKey": uuid::Uuid::new_v4().to_string(),
        }),
    )
    .await?;
    result["itemId"]
        .as_str()
        .map(str::to_string)
        .context("task/start itemId")
}

async fn read_task(
    runtime: &Arc<devo_server::ServerRuntime>,
    connection_id: u64,
    item_id: &str,
) -> Result<Value> {
    native_rpc(
        runtime,
        connection_id,
        "task/read",
        json!({ "itemId": item_id }),
    )
    .await
}

async fn wait_for_task_terminal(
    runtime: &Arc<devo_server::ServerRuntime>,
    connection_id: u64,
    item_id: &str,
) -> Result<Value> {
    timeout(Duration::from_secs(10), async {
        loop {
            let result = read_task(runtime, connection_id, item_id).await?;
            if result["item"]["state"].as_str() != Some("running") {
                return Ok(result);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .context("timed out waiting for task to finish")?
}

async fn wait_for_task_output(
    runtime: &Arc<devo_server::ServerRuntime>,
    connection_id: u64,
    item_id: &str,
    marker: &str,
) -> Result<Value> {
    timeout(Duration::from_secs(5), async {
        loop {
            let result = read_task(runtime, connection_id, item_id).await?;
            if result["outputTail"]
                .as_str()
                .is_some_and(|output| output.contains(marker))
            {
                return Ok(result);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .context("timed out waiting for task output")?
}

async fn list_tasks(
    runtime: &Arc<devo_server::ServerRuntime>,
    connection_id: u64,
    session_id: devo_protocol::SessionId,
) -> Result<Value> {
    native_rpc(
        runtime,
        connection_id,
        "task/list",
        json!({ "sessionId": session_id }),
    )
    .await
}

async fn write_task_stdin(
    runtime: &Arc<devo_server::ServerRuntime>,
    connection_id: u64,
    item_id: &str,
    data: &str,
) -> Result<()> {
    native_rpc(
        runtime,
        connection_id,
        "task/write_stdin",
        json!({ "itemId": item_id, "data": data }),
    )
    .await?;
    Ok(())
}

async fn interrupt_task(
    runtime: &Arc<devo_server::ServerRuntime>,
    connection_id: u64,
    item_id: &str,
) -> Result<()> {
    native_rpc(
        runtime,
        connection_id,
        "task/interrupt",
        json!({ "itemId": item_id }),
    )
    .await?;
    Ok(())
}

#[cfg(unix)]
fn interactive_command() -> &'static str {
    "printf 'ready\\n'; IFS= read -r line; printf 'received:%s\\n' \"$line\"; printf 'done\\n'"
}

#[cfg(windows)]
fn interactive_command() -> &'static str {
    "Write-Output 'ready'; $line = Read-Host; Write-Output \"received:$line\"; Write-Output 'done'"
}

#[cfg(unix)]
fn waiting_background_command() -> &'static str {
    "printf 'background-started\\n'; IFS= read -r line; printf 'background-done:%s\\n' \"$line\""
}

#[cfg(windows)]
fn waiting_background_command() -> &'static str {
    "Write-Output 'background-started'; $line = Read-Host; Write-Output \"background-done:$line\""
}

#[cfg(unix)]
fn cancel_background_command() -> &'static str {
    "printf 'background-started\\n'; sleep 30; printf 'should-not-finish\\n'"
}

#[cfg(windows)]
fn cancel_background_command() -> &'static str {
    "Write-Output 'background-started'; Start-Sleep -Seconds 30; Write-Output 'should-not-finish'"
}

#[tokio::test]
async fn native_task_process_accepts_stdin_and_exits() -> Result<()> {
    let (data_root, runtime, connection_id, session_id) = setup_runtime().await?;
    let item_id = start_process(
        &runtime,
        connection_id,
        session_id,
        data_root.path(),
        interactive_command(),
    )
    .await?;
    let started = read_task(&runtime, connection_id, &item_id).await?;
    assert_eq!(started["item"]["state"], "running");

    write_task_stdin(&runtime, connection_id, &item_id, "hello\n").await?;
    let finished = wait_for_task_terminal(&runtime, connection_id, &item_id).await?;
    assert_eq!(finished["item"]["state"], "completed");
    let output = finished["outputTail"]
        .as_str()
        .context("task output tail")?;
    assert!(output.contains("ready"));
    assert!(output.contains("received:hello"));
    assert!(output.contains("done"));

    Ok(())
}

#[tokio::test]
async fn native_task_list_and_read_track_background_process() -> Result<()> {
    let (data_root, runtime, connection_id, session_id) = setup_runtime().await?;
    let item_id = start_process(
        &runtime,
        connection_id,
        session_id,
        data_root.path(),
        waiting_background_command(),
    )
    .await?;

    let listed = list_tasks(&runtime, connection_id, session_id).await?;
    let task = listed["tasks"]
        .as_array()
        .and_then(|tasks| tasks.iter().find(|task| task["id"] == item_id))
        .context("running task in task/list")?;
    assert_eq!(task["state"], "running");

    write_task_stdin(&runtime, connection_id, &item_id, "finish\n").await?;
    let finished = wait_for_task_terminal(&runtime, connection_id, &item_id).await?;
    assert_eq!(finished["item"]["state"], "completed");
    let output = finished["outputTail"]
        .as_str()
        .context("task output tail")?;
    assert!(output.contains("background-started"));
    assert!(output.contains("background-done:finish"));

    Ok(())
}

#[tokio::test]
async fn native_task_interrupt_stops_process_and_retains_terminal_entry() -> Result<()> {
    let (data_root, runtime, connection_id, session_id) = setup_runtime().await?;
    let item_id = start_process(
        &runtime,
        connection_id,
        session_id,
        data_root.path(),
        cancel_background_command(),
    )
    .await?;
    let started =
        wait_for_task_output(&runtime, connection_id, &item_id, "background-started").await?;
    assert_eq!(started["item"]["state"], "running");

    interrupt_task(&runtime, connection_id, &item_id).await?;
    let finished = wait_for_task_terminal(&runtime, connection_id, &item_id).await?;
    assert_eq!(finished["item"]["state"], "completed");
    let output = finished["outputTail"].as_str().unwrap_or_default();
    assert!(output.contains("background-started"));
    assert!(!output.contains("should-not-finish"));

    let listed = list_tasks(&runtime, connection_id, session_id).await?;
    assert!(listed["tasks"].as_array().is_some_and(|tasks| {
        tasks
            .iter()
            .any(|task| task["id"] == item_id && task["state"] == "completed")
    }));

    Ok(())
}

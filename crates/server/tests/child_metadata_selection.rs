//! Strict settings updates for ephemeral children without SQLite metadata.
#[path = "support/subagent_lifecycle.rs"]
mod support;

use anyhow::{Context, Result};
use devo_core::tools::AgentToolCoordinator;
use devo_protocol::{ProviderWireApi, SpawnAgentParams};
use devo_provider::{MultiProviderRouter, ProviderRoute};
use devo_server::test_support::TestRuntime;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::sync::Arc;
use support::*;
use tempfile::TempDir;

#[tokio::test]
async fn ephemeral_child_metadata_rejects_fallback_without_changing_defaults() -> Result<()> {
    let home = TempDir::new()?;
    let providers = json!({"provider": {"local": {
        "wire_api": "openai_chat_completions", "base_url": "https://local.invalid/v1",
        "models": {
            "main": {"reasoning_capability": {"levels": ["off", "high"]}},
            "other": {"reasoning_capability": {"levels": ["off", "high"]}}
        }
    }}});
    std::fs::write(
        home.path().join("providers.json"),
        serde_json::to_vec(&providers)?,
    )?;
    let provider = Arc::new(ScriptedProvider::pending());
    let mut router = MultiProviderRouter::new(provider.clone());
    router.insert_route(
        ProviderRoute::connection("local", ProviderWireApi::OpenAIChatCompletions),
        provider.clone(),
    );
    let runtime = TestRuntime::new(provider.clone())
        .router(Arc::new(router))
        .default_model("local/main")
        .runtime(home.path());
    let (connection, _notifications) = initialize_connection(&runtime).await?;
    let parent = start_parent_session(&runtime, connection, home.path()).await?;
    let params: SpawnAgentParams = serde_json::from_value(json!({
        "session_id": parent, "message": "Keep working", "ephemeral": true,
        "model": "local/main", "thinking": "high"
    }))?;
    let child = Arc::clone(&runtime).spawn_agent(params).await?;
    wait_for_stream_calls(&provider, 1).await?;
    let config_before = std::fs::read(home.path().join("config.toml")).ok();
    for (patch, expected_error) in [
        (
            json!({"model": {"provider": "local", "model": "missing"}}),
            "child model is not configured",
        ),
        (
            json!({"settings": {"reasoningEffort": "impossible"}}),
            "unsupported for child model",
        ),
    ] {
        let mut params = patch;
        params["sessionId"] = json!(child.child_session_id);
        params["expectedVersion"] = json!(0);
        let response = runtime
            .handle_incoming(
                connection,
                json!({
                    "id": 50, "method": "session/metadata/update", "params": params
                }),
            )
            .await
            .context("invalid metadata response")?;
        assert!(response.get("error").is_some(), "{response}");
        assert!(response.to_string().contains(expected_error), "{response}");
        assert!(
            !response.to_string().contains("cannot be updated directly"),
            "test must exercise ephemeral validation, not indexed-child rejection"
        );
    }
    let response = runtime
        .handle_incoming(
            connection,
            json!({
                "id": 51, "method": "session/metadata/update", "params": {
                    "sessionId": child.child_session_id, "expectedVersion": 0,
                    "model": {"provider": "local", "model": "other"},
                    "settings": {"reasoningEffort": "off"}
                }
            }),
        )
        .await
        .context("valid metadata response")?;
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(response["result"]["appliedToActiveTurn"], json!(true));
    assert_eq!(
        response["result"]["session"]["model"]["model"],
        json!("local/other")
    );
    assert_eq!(
        response["result"]["session"]["settings"]["reasoningEffort"],
        json!("off")
    );
    assert_eq!(
        std::fs::read(home.path().join("config.toml")).ok(),
        config_before
    );
    assert_eq!(provider.requests().len(), 1);
    Ok(())
}

//! Native preference choices must use workspace configuration, not catalog placeholders.

use std::pin::Pin;
use std::sync::Arc;

use anyhow::{Context, Result};
use devo_protocol::{ModelRequest, ModelResponse, StreamEvent};
use devo_provider::ModelProviderSDK;
use devo_server::test_support::TestRuntime;
use devo_server::{ClientTransportKind, ServerRuntime};
use futures::Stream;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

#[derive(Clone, Copy)]
enum DefaultProvider {
    Available,
    Missing,
}

#[async_trait::async_trait]
impl ModelProviderSDK for DefaultProvider {
    fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }

    async fn completion(&self, _: ModelRequest) -> Result<ModelResponse> {
        panic!("reading model preferences must not invoke a model")
    }

    async fn completion_stream(
        &self,
        _: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        panic!("reading model preferences must not invoke a model")
    }

    fn name(&self) -> &str {
        "model-preferences-test"
    }
}

async fn request(
    runtime: &Arc<ServerRuntime>,
    connection: u64,
    method: &str,
    params: Value,
) -> Result<Value> {
    let response = runtime
        .handle_incoming(
            connection,
            json!({ "id": 1, "method": method, "params": params }),
        )
        .await
        .context("missing RPC response")?;
    anyhow::ensure!(response.get("error").is_none(), "{method}: {response}");
    Ok(response["result"].clone())
}

async fn connect(runtime: &Arc<ServerRuntime>) -> Result<u64> {
    let (outbound, _incoming) = devo_server::test_outbound_channel(4096);
    let connection = runtime
        .register_connection(ClientTransportKind::Stdio, outbound)
        .await;
    request(
        runtime,
        connection,
        "initialize",
        json!({
            "protocolVersion": 1, "clientCapabilities": {},
            "_meta": { "devo": { "protocol": "native" } },
        }),
    )
    .await?;
    Ok(connection)
}

#[tokio::test]
async fn missing_provider_keeps_current_preference_without_offering_placeholder_as_available()
-> Result<()> {
    let home = tempfile::tempdir()?;
    let runtime = TestRuntime::new(Arc::new(DefaultProvider::Missing))
        .with_named_model("302ai/MiniMax-M1", "MiniMax-M1")
        .default_model("302ai/MiniMax-M1")
        .runtime(home.path());
    let connection = connect(&runtime).await?;
    let read = request(&runtime, connection, "model/preferences/read", json!({})).await?;
    assert_eq!(read["preferences"]["model"], json!("302ai/MiniMax-M1"));
    assert_eq!(read["preferences"]["availableModels"], json!([]));
    let sessions = request(&runtime, connection, "session/list", json!({})).await?;
    assert_eq!(sessions["data"], json!([]));
    Ok(())
}

#[tokio::test]
async fn available_default_routes_keep_qualified_and_unqualified_model_choices() -> Result<()> {
    for slug in ["default-model", "organization/model"] {
        let home = tempfile::tempdir()?;
        let runtime = TestRuntime::new(Arc::new(DefaultProvider::Available))
            .with_named_model(slug, "Default model")
            .default_model(slug)
            .runtime(home.path());
        let connection = connect(&runtime).await?;
        let read = request(&runtime, connection, "model/preferences/read", json!({})).await?;
        let values: Vec<_> = read["preferences"]["availableModels"]
            .as_array()
            .context("availableModels")?
            .iter()
            .map(|option| option["value"].clone())
            .collect();
        assert_eq!(values, vec![json!(slug)]);
    }
    Ok(())
}

#[tokio::test]
async fn workspace_only_connection_remains_available_without_global_connection_or_credential()
-> Result<()> {
    let home = tempfile::tempdir()?;
    std::fs::write(
        home.path().join("config.toml"),
        "[catalog]\noffline = true\n",
    )?;
    let workspace = tempfile::tempdir()?;
    std::fs::create_dir(workspace.path().join(".devo"))?;
    std::fs::write(
        workspace.path().join(".devo/providers.json"),
        serde_json::to_vec(&json!({
            "model": "reviewlocal/tiny", "provider": { "reviewlocal": {
                "name": "Workspace Local", "enabled": true, "base_url": "http://127.0.0.1:1/v1",
                "wire_api": "openai_chat_completions", "models": { "tiny": { "name": "Tiny" } },
            } },
        }))?,
    )?;
    let runtime = TestRuntime::new(Arc::new(DefaultProvider::Missing))
        .with_named_model("302ai/MiniMax-M1", "MiniMax-M1")
        .default_model("302ai/MiniMax-M1")
        .runtime(home.path());
    let connection = connect(&runtime).await?;
    let catalog = request(&runtime, connection, "provider/list", json!({})).await?;
    let credentials = request(&runtime, connection, "credential/list", json!({})).await?;
    assert_eq!(catalog["connectedProviderIds"], json!([]));
    assert_eq!(credentials["credentials"], json!([]));
    let read = request(
        &runtime,
        connection,
        "model/preferences/read",
        json!({ "cwd": workspace.path() }),
    )
    .await?;
    let values: Vec<_> = read["preferences"]["availableModels"]
        .as_array()
        .context("availableModels")?
        .iter()
        .map(|option| option["value"].clone())
        .collect();
    assert_eq!(values, vec![json!("reviewlocal/tiny")]);
    assert_eq!(read["preferences"]["model"], json!("reviewlocal/tiny"));
    Ok(())
}

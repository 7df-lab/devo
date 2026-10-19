//! Session effective context window uses the model usable window only.
//! Legacy `compaction_token_limit` in config.toml and `effectiveContextWindow`
//! patches do not change applied policy.

use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use devo_core::PresetModelCatalog;
use devo_protocol::SessionId;
use devo_server::ClientTransportKind;
use devo_server::ServerRuntime;
use devo_server::SuccessResponse;
use devo_server::test_support::NoopProvider;
use devo_server::test_support::TestRuntime;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

fn build_runtime(data_root: &Path) -> Result<Arc<ServerRuntime>> {
    Ok(TestRuntime::new(Arc::new(
        NoopProvider::text().named("noop-compaction-threshold-provider"),
    ))
    .catalog(Arc::new(PresetModelCatalog::load()?))
    .db_file("compaction_threshold.db")
    .runtime(data_root))
}

async fn initialize_connection(runtime: &Arc<ServerRuntime>) -> Result<u64> {
    let (notifications_tx, _notifications_rx) = devo_server::test_outbound_channel(128);
    let connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, notifications_tx)
        .await;
    let initialize_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": 1,
                    "clientCapabilities": {},
                    "_meta": { "devo": { "protocol": "native" } },
                    "clientInfo": {
                        "name": "compaction-threshold-test",
                        "title": "Compaction Threshold Test",
                        "version": "1.0.0"
                    }
                }
            }),
        )
        .await
        .context("initialize response")?;
    assert_eq!(
        initialize_response["result"]["agentInfo"]["name"],
        serde_json::json!("devo-server")
    );
    Ok(connection_id)
}

async fn start_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    cwd: &Path,
    idempotency_key: &str,
) -> Result<devo_protocol::native::rpc_session::SessionNewResult> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 2,
                "method": "session/new",
                "params": {
                    "cwd": cwd,
                    "idempotencyKey": idempotency_key
                }
            }),
        )
        .await
        .context("session/new response")?;
    let response: SuccessResponse<devo_protocol::native::rpc_session::SessionNewResult> =
        serde_json::from_value(response)?;
    Ok(response.result)
}

async fn compaction_update(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: SessionId,
    effective_context_window: u64,
) -> Result<devo_protocol::native::rpc_session::SessionMetadataUpdateResult> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 3,
                "method": "session/metadata/update",
                "params": {
                    "sessionId": session_id,
                    "expectedVersion": 0,
                    "settings": { "effectiveContextWindow": effective_context_window }
                }
            }),
        )
        .await
        .context("session/metadata/update response")?;
    let response: SuccessResponse<devo_protocol::native::rpc_session::SessionMetadataUpdateResult> =
        serde_json::from_value(response)?;
    Ok(response.result)
}

#[tokio::test]
async fn effective_context_window_patch_echoes_model_and_skips_global_config() -> Result<()> {
    let data_root = TempDir::new()?;
    let cwd = data_root.path().join("workspace");
    std::fs::create_dir_all(&cwd)?;
    let runtime = build_runtime(data_root.path())?;
    let connection_id = initialize_connection(&runtime).await?;
    let started = start_session(
        &runtime,
        connection_id,
        &cwd,
        "compaction-threshold-session-1",
    )
    .await?;
    let started_id = SessionId::from(started.session.id.as_str());
    let model_default = started
        .session
        .settings
        .effective_context_window
        .expect("new session should expose an applied effective window");

    let updated = compaction_update(
        &runtime,
        connection_id,
        started_id,
        /*effective_context_window*/ 250_000,
    )
    .await?;
    assert_eq!(
        updated.session.settings.effective_context_window,
        Some(model_default),
        "patch must echo model effective window, not the requested absolute"
    );

    let config_path = data_root.path().join("config.toml");
    if config_path.exists() {
        let config_text = std::fs::read_to_string(&config_path)?;
        let document: toml::Value = toml::from_str(&config_text)?;
        assert!(
            document.get("compaction_token_limit").is_none(),
            "effectiveContextWindow must not write the removed compaction_token_limit key"
        );
    }

    let second = start_session(
        &runtime,
        connection_id,
        &cwd,
        "compaction-threshold-session-2",
    )
    .await?;
    assert_eq!(
        second.session.settings.effective_context_window,
        Some(model_default),
        "new sessions keep the model effective window"
    );
    Ok(())
}

#[tokio::test]
async fn new_session_ignores_legacy_compaction_token_limit_in_toml() -> Result<()> {
    let data_root = TempDir::new()?;
    let cwd = data_root.path().join("workspace");
    std::fs::create_dir_all(&cwd)?;
    // Unknown keys are ignored by serde; this proves a leftover user config
    // key does not affect the applied model-derived window.
    std::fs::write(
        data_root.path().join("config.toml"),
        "compaction_token_limit = 100000\n",
    )?;

    let runtime = build_runtime(data_root.path())?;
    let connection_id = initialize_connection(&runtime).await?;
    let started = start_session(
        &runtime,
        connection_id,
        &cwd,
        "compaction-threshold-session-existing",
    )
    .await?;
    let applied = started
        .session
        .settings
        .effective_context_window
        .expect("new session should expose an applied effective window");
    assert_ne!(
        applied, 100_000,
        "legacy compaction_token_limit in config.toml must not become the applied window"
    );
    Ok(())
}

#[path = "support/acp_runtime_harness.rs"]
mod acp_runtime_harness;

use anyhow::Context;
use anyhow::Result;
use devo_protocol::AcpNewSessionResult;
use devo_protocol::AcpSessionNotification;
use devo_protocol::AcpSessionUpdate;
use devo_protocol::SessionId;
use devo_server::ClientTransportKind;
use devo_server::OutboundFrame;
use devo_server::test_support::TestRuntime;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio::time::Duration;
use tokio::time::timeout;

use acp_runtime_harness::assert_available_command_names;
use acp_runtime_harness::path_value;

#[tokio::test]
async fn acp_available_commands_are_session_update_after_session_response() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = TestRuntime::noop_empty()
        .with_test_model()
        .db_file("acp_available_commands.db")
        .runtime(data_root.path());
    let (outgoing_tx, mut outgoing_rx) = devo_server::test_outbound_channel(4096);
    let connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, outgoing_tx.clone())
        .await;
    runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": 1,
                    "clientCapabilities": {},
                    "clientInfo": {
                        "name": "acp-available-commands-order-test",
                        "title": "ACP Available Commands Order Test",
                        "version": "1.0.0"
                    }
                }
            }),
        )
        .await
        .context("initialize response")?;

    let cwd = data_root.path().join("workspace");
    std::fs::create_dir_all(&cwd)?;
    let incoming_response = runtime
        .handle_incoming_with_actions(
            connection_id,
            serde_json::json!({
                "id": 2,
                "method": "session/new",
                "params": { "cwd": path_value(&cwd), "mcpServers": [] }
            }),
        )
        .await
        .context("session/new response")?;
    let (response_value, post_response_actions) = incoming_response.into_parts();
    let response: devo_server::AcpSuccessResponse<AcpNewSessionResult> =
        serde_json::from_value(response_value.clone())?;
    let session_id = response.result.session_id;
    outgoing_tx
        .send(OutboundFrame::json_rpc_response(
            connection_id,
            response_value,
        ))
        .await
        .context("enqueue simulated transport response")?;
    runtime
        .run_post_response_actions(post_response_actions)
        .await;

    let messages_before_response = recv_until_response(&mut outgoing_rx, 2, session_id).await?;
    assert!(
        !messages_before_response
            .iter()
            .any(|message| is_available_commands_update(message, session_id)),
        "available_commands_update arrived before response: {messages_before_response:?}"
    );

    let message = recv_available_commands_update(&mut outgoing_rx, session_id).await?;
    let notification: AcpSessionNotification = serde_json::from_value(message["params"].clone())?;
    if let AcpSessionUpdate::AvailableCommandsUpdate {
        available_commands, ..
    } = notification.update
    {
        assert_available_command_names(&available_commands)?;
    } else {
        anyhow::bail!("expected available_commands_update");
    }
    Ok(())
}

async fn recv_until_response(
    outgoing_rx: &mut mpsc::Receiver<serde_json::Value>,
    request_id: u64,
    session_id: SessionId,
) -> Result<Vec<serde_json::Value>> {
    timeout(Duration::from_secs(5), async {
        let mut messages = Vec::new();
        loop {
            let message = outgoing_rx
                .recv()
                .await
                .context("outgoing channel closed before response")?;
            if message.get("id") == Some(&serde_json::json!(request_id)) {
                return Ok(messages);
            }
            if message["params"]["sessionId"].as_str() == Some(session_id.as_ref()) {
                messages.push(message);
            }
        }
    })
    .await
    .context("timed out waiting for response")?
}

async fn recv_available_commands_update(
    outgoing_rx: &mut mpsc::Receiver<serde_json::Value>,
    session_id: SessionId,
) -> Result<serde_json::Value> {
    timeout(Duration::from_secs(5), async {
        loop {
            let message = outgoing_rx
                .recv()
                .await
                .context("outgoing channel closed before available commands update")?;
            if is_available_commands_update(&message, session_id) {
                return Ok(message);
            }
        }
    })
    .await
    .context("timed out waiting for available commands update")?
}

fn is_available_commands_update(message: &serde_json::Value, session_id: SessionId) -> bool {
    message["method"] == serde_json::json!("session/update")
        && message["params"]["sessionId"].as_str() == Some(session_id.as_ref())
        && message["params"]["update"]["sessionUpdate"].as_str()
            == Some("available_commands_update")
}

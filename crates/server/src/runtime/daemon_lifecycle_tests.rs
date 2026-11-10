//! Admission must remain tracked after the initiating client disappears.
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use futures::{SinkExt, StreamExt};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;

use crate::transport::{InternalProxyControl, InternalProxyEndpoint, run_daemon_listener};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

async fn response(socket: &mut Socket, id: u64) -> Result<Value> {
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(frame) = socket.next().await {
            let frame = frame?;
            if let Message::Text(text) = frame {
                let value: Value = serde_json::from_str(&text)?;
                if value["id"] == id {
                    anyhow::ensure!(value.get("error").is_none(), "RPC failed: {value}");
                    return Ok(value["result"].clone());
                }
            }
        }
        anyhow::bail!("socket closed")
    })
    .await?
}

#[tokio::test]
async fn daemon_stop_waits_for_rpc_admitted_by_already_disconnected_client() -> Result<()> {
    let home = tempfile::tempdir()?;
    let runtime = crate::TestRuntime::noop().runtime(home.path());
    let endpoint = InternalProxyEndpoint::bind().await?;
    let url = endpoint.endpoint().to_string();
    let cancel = CancellationToken::new();
    let mut listener = tokio::spawn(run_daemon_listener(
        Arc::clone(&runtime),
        endpoint,
        "test-token".into(),
        InternalProxyControl::daemon(cancel.clone()),
    ));
    let (mut socket, _) = connect_async(url).await?;
    socket.send(Message::Text("test-token".into())).await?;
    socket.send(Message::Text(json!({"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":1,"clientCapabilities":{},"_meta":{"devo":{"protocol":"native"}}}
    }).to_string().into())).await?;
    response(&mut socket, 1).await?;
    socket
        .send(Message::Text(
            json!({"jsonrpc":"2.0","id":2,"method":"session/new",
                "params":{"cwd":home.path(),"idempotencyKey":"drain-fixture"}
            })
            .to_string()
            .into(),
        ))
        .await?;
    let created = response(&mut socket, 2).await?;
    let session_id = serde_json::from_value(created["session"]["id"].clone())?;
    let gate = runtime
        .session_metadata_write_gate
        .acquire(session_id)
        .await;
    socket.send(Message::Text(json!({"jsonrpc":"2.0","id":3,"method":"session/metadata/update",
        "params":{"sessionId":session_id,"expectedVersion":0,"title":"admitted before disconnect"}
    }).to_string().into())).await?;
    // The following response proves the reader admitted request 3 first. Its
    // handler remains blocked at the metadata gate while ping stays responsive.
    socket
        .send(Message::Text(
            json!({"jsonrpc":"2.0","id":4,"method":"runtime/ping","params":{}})
                .to_string()
                .into(),
        ))
        .await?;
    response(&mut socket, 4).await?;
    socket.close(None).await?;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !runtime.connections.lock().await.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("client unregister")?;
    cancel.cancel();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut listener)
            .await
            .is_err()
    );
    drop(gate);
    tokio::time::timeout(Duration::from_secs(5), &mut listener).await???;
    assert_eq!(
        runtime
            .deps
            .db
            .get_session_index(&session_id)?
            .context("index")?
            .session
            .title,
        Some("admitted before disconnect".into())
    );
    runtime.shutdown().await;
    Ok(())
}

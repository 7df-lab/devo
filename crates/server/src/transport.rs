use std::sync::Arc;

use anyhow::Result;
use anyhow::anyhow;
use async_trait::async_trait;
use devo_protocol::ErrorResponse;
use devo_protocol::NotificationEnvelope;
use devo_protocol::SuccessResponse;
use futures::SinkExt;
use futures::StreamExt;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

use crate::ClientTransportKind;
use crate::ServerRuntime;

/// Transport trait per L3-BEH-SERVER-001.
///
/// Abstracts the connection to a client, allowing different transport
/// implementations (stdio, WebSocket, etc.) to be used interchangeably.
#[async_trait]
pub trait Transport: Send + Sync {
    /// Send a success response to the client.
    async fn send_response(&self, response: SuccessResponse<serde_json::Value>);

    /// Send an error response to the client.
    async fn send_error(&self, error: ErrorResponse);

    /// Send a notification to the client.
    async fn send_notification(&self, notification: NotificationEnvelope<serde_json::Value>);

    /// Close the transport connection.
    async fn close(&self);
}

/// EventBroadcaster per L3-BEH-SERVER-001.
///
/// Manages event delivery to connected clients with monotonic per-session
/// sequence numbering and subscription filtering.
pub struct EventBroadcaster {
    /// Per-connection senders, keyed by connection_id.
    connections: Arc<
        tokio::sync::RwLock<
            std::collections::HashMap<u64, mpsc::UnboundedSender<serde_json::Value>>,
        >,
    >,
    /// Per-session monotonic sequence counters.
    sequence_counters: Arc<tokio::sync::RwLock<std::collections::HashMap<String, u64>>>,
}

impl EventBroadcaster {
    pub fn new() -> Self {
        Self {
            connections: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            sequence_counters: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        }
    }

    /// Register a connection's event sender.
    pub async fn register(
        &self,
        connection_id: u64,
        sender: mpsc::UnboundedSender<serde_json::Value>,
    ) {
        self.connections.write().await.insert(connection_id, sender);
    }

    /// Unregister a connection.
    pub async fn unregister(&self, connection_id: u64) {
        self.connections.write().await.remove(&connection_id);
    }

    /// Broadcast an event to all connected clients.
    /// Returns the next sequence number for the session.
    pub async fn broadcast(&self, session_id: &str, event: serde_json::Value) -> u64 {
        let mut counters = self.sequence_counters.write().await;
        let seq = counters.entry(session_id.to_string()).or_insert(0);
        *seq += 1;
        let current_seq = *seq;
        drop(counters);

        let connections = self.connections.read().await;
        for sender in connections.values() {
            let _ = sender.send(event.clone());
        }
        current_seq
    }

    /// Get the current sequence number for a session.
    pub async fn current_sequence(&self, session_id: &str) -> u64 {
        self.sequence_counters
            .read()
            .await
            .get(session_id)
            .copied()
            .unwrap_or(0)
    }

    /// Get the number of active connections.
    pub async fn connection_count(&self) -> usize {
        self.connections.read().await.len()
    }
}

impl Default for EventBroadcaster {
    fn default() -> Self {
        Self::new()
    }
}

/// Default bind address used when the WebSocket transport is selected without
/// an explicit host-and-port suffix.
pub const DEFAULT_WEBSOCKET_BIND_ADDRESS: &str = "127.0.0.1:3210";

/// Enumerates the supported listener targets parsed from server config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListenTarget {
    /// Start the process-scoped stdio transport.
    Stdio,
    /// Start a WebSocket listener at one host and port pair.
    WebSocket {
        /// The socket address host and port, without the `ws://` prefix.
        bind_address: String,
    },
}

/// Parses one configured listen-address string into a typed transport target.
pub fn parse_listen_target(value: &str) -> Result<ListenTarget> {
    if value.eq_ignore_ascii_case("stdio://") || value.eq_ignore_ascii_case("stdio") {
        return Ok(ListenTarget::Stdio);
    }
    if let Some(bind_address) = value.strip_prefix("ws://") {
        return Ok(ListenTarget::WebSocket {
            bind_address: if bind_address.is_empty() {
                DEFAULT_WEBSOCKET_BIND_ADDRESS.to_string()
            } else {
                bind_address.to_string()
            },
        });
    }
    Err(anyhow!("unsupported listen target: {value}"))
}

/// Resolves the configured listen-address strings into the concrete listener
/// targets the process will start.
pub fn resolve_listen_targets(listen: &[String]) -> Result<Vec<ListenTarget>> {
    if listen.is_empty() {
        Ok(vec![
            ListenTarget::Stdio,
            ListenTarget::WebSocket {
                bind_address: DEFAULT_WEBSOCKET_BIND_ADDRESS.to_string(),
            },
        ])
    } else {
        listen
            .iter()
            .map(|value| parse_listen_target(value))
            .collect::<Result<Vec<_>>>()
    }
}

/// Runs every configured listener target until shutdown.
pub async fn run_listeners(runtime: Arc<ServerRuntime>, listen: &[String]) -> Result<()> {
    let targets = resolve_listen_targets(listen)?;

    let mut tasks = Vec::new();
    for target in targets {
        let runtime = Arc::clone(&runtime);
        tasks.push(tokio::spawn(async move {
            match target {
                ListenTarget::Stdio => {
                    tracing::info!("stdio listener active on stdin/stdout");
                    run_stdio(runtime).await
                }
                ListenTarget::WebSocket { bind_address } => {
                    tracing::info!(bind_address = %bind_address, "websocket listener starting");
                    run_websocket(runtime, &bind_address).await
                }
            }
        }));
    }

    for task in tasks {
        task.await??;
    }
    Ok(())
}

async fn run_stdio(runtime: Arc<ServerRuntime>) -> Result<()> {
    // Normal channel for event notifications (TextDelta, TurnStarted, …).
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let sender_clone = sender.clone();
    let connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, sender)
        .await;
    tracing::info!(connection_id, "stdio connection established");

    // Internal channel between the producer (reads from high_pri + normal)
    // and the writer (writes to stdout). Unbounded so the producer never
    // blocks — if stdout is slow, messages queue here instead of blocking
    // the select loop that processes RPC responses.
    let (write_tx, mut write_rx) = mpsc::unbounded_channel::<Vec<u8>>();

    // --- Writer task ---
    // Sole responsibility: read serialized lines from write_rx and write
    // them to stdout. This is the only task that can block on stdout.
    let writer_task = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(line) = write_rx.recv().await {
            stdout.write_all(&line).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
        Result::<()>::Ok(())
    });

    // --- Producer task ---
    // Reads from high_pri and normal channels, serializes immediately,
    // and pushes to write_tx. Never blocks on stdout.
    let producer_task = tokio::spawn(async move {
        loop {
            let line: Vec<u8>;
            tokio::select! {
                Some(message) = receiver.recv() => {
                    line = serde_json::to_vec(&message)
                        .expect("serialize stdio response");
                }
                else => break,
            }
            if write_tx.send(line).is_err() {
                break;
            }
        }
    });

    // --- Stdin reader (main task) ---
    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(&line)?;
        if let Some(response) = runtime.handle_incoming(connection_id, value).await {
            let _ = sender_clone.send(response);
        }
    }

    runtime.unregister_connection(connection_id).await;
    tracing::info!(connection_id, "stdio connection closed");
    writer_task.abort();
    producer_task.abort();
    Ok(())
}

async fn run_websocket(runtime: Arc<ServerRuntime>, bind_address: &str) -> Result<()> {
    let listener = TcpListener::bind(bind_address).await?;
    tracing::info!(bind_address = %bind_address, "websocket listener bound");
    loop {
        let (stream, remote_addr) = listener.accept().await?;
        let runtime = Arc::clone(&runtime);
        tokio::spawn(async move {
            tracing::info!(remote_addr = %remote_addr, "websocket client connected");
            if let Err(error) = handle_websocket_connection(runtime, stream).await {
                tracing::warn!(remote_addr = %remote_addr, error = %error, "websocket connection closed with error");
            }
            tracing::info!(remote_addr = %remote_addr, "websocket client disconnected");
        });
    }
}

async fn handle_websocket_connection(
    runtime: Arc<ServerRuntime>,
    stream: tokio::net::TcpStream,
) -> Result<()> {
    let websocket = accept_async(stream).await?;
    let (mut writer, mut reader) = websocket.split();
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let sender_clone = sender.clone();
    let connection_id = runtime
        .register_connection(ClientTransportKind::WebSocket, sender)
        .await;
    tracing::info!(connection_id, "websocket connection established");

    let writer_task = tokio::spawn(async move {
        while let Some(message) = receiver.recv().await {
            writer
                .send(Message::Text(
                    serde_json::to_string(&message)
                        .expect("serialize websocket response")
                        .into(),
                ))
                .await?;
        }
        Result::<()>::Ok(())
    });

    while let Some(frame) = reader.next().await {
        let frame = frame?;
        match frame {
            Message::Text(text) => {
                let value: serde_json::Value = serde_json::from_str(&text)?;
                if let Some(response) = runtime.handle_incoming(connection_id, value).await {
                    let _ = sender_clone.send(response);
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    runtime.unregister_connection(connection_id).await;
    tracing::info!(connection_id, "websocket connection closed");
    writer_task.abort();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::DEFAULT_WEBSOCKET_BIND_ADDRESS;
    use super::ListenTarget;
    use super::parse_listen_target;
    use super::resolve_listen_targets;

    #[test]
    fn parse_stdio_target() {
        assert_eq!(
            parse_listen_target("stdio://").expect("stdio"),
            ListenTarget::Stdio
        );
    }

    #[test]
    fn parse_ws_target() {
        assert_eq!(
            parse_listen_target("ws://127.0.0.1:9000").expect("ws"),
            ListenTarget::WebSocket {
                bind_address: "127.0.0.1:9000".into(),
            }
        );
    }

    #[test]
    fn parse_ws_target_without_bind_address_uses_default() {
        assert_eq!(
            parse_listen_target("ws://").expect("ws"),
            ListenTarget::WebSocket {
                bind_address: DEFAULT_WEBSOCKET_BIND_ADDRESS.into(),
            }
        );
    }

    #[test]
    fn resolve_empty_listener_list_defaults_to_stdio() {
        assert_eq!(
            resolve_listen_targets(&[]).expect("targets"),
            vec![
                ListenTarget::Stdio,
                ListenTarget::WebSocket {
                    bind_address: DEFAULT_WEBSOCKET_BIND_ADDRESS.into(),
                },
            ]
        );
    }
}

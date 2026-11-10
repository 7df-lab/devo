//! Singleton server coordination for a single `DEVO_HOME`.
//!
//! At most one **real** devo-server process may run per user data directory.
//! Coordination uses an exclusive file lock plus a small JSON metadata file:
//!
//! - `server.lock` — held for the lifetime of the real server (`RealServerGuard`).
//! - `server.lock.json` — written after the real server binds its internal proxy;
//!   contains pid, loopback WebSocket URL, and a random auth token.
//!
//! A second process that fails to acquire the lock becomes a **proxy** client:
//! stdio mode forwards stdin/stdout through the internal proxy WebSocket
//! (`run_stdio_proxy`); `--status` / `--shutdown` use one-shot control RPCs
//! (`run_server_control`). See `bootstrap::run_server_process` for the full flow.

use std::fs;
use std::fs::File;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use chrono::Utc;
use fs2::FileExt;
use futures::SinkExt;
use futures::StreamExt;
use serde::Deserialize;
use serde::Serialize;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

use crate::ProtocolSet;
use crate::ServerProtocol;

const LOCK_FILE_NAME: &str = "server.lock";
const METADATA_FILE_NAME: &str = "server.lock.json";
const METADATA_VERSION: u32 = 1;
/// Real server may still be writing metadata when a proxy process starts.
const METADATA_READ_RETRIES: usize = 100;
const METADATA_READ_RETRY_DELAY: Duration = Duration::from_millis(50);
pub(crate) const SERVER_CONTROL_STATUS_METHOD: &str = "server/status";
pub(crate) const SERVER_CONTROL_SHUTDOWN_METHOD: &str = "server/shutdown";
pub(crate) const DAEMON_CONTROL_SHUTDOWN_METHOD: &str = "daemon/shutdown";
pub(crate) const SERVER_CONTROL_ENABLE_PROTOCOLS_METHOD: &str = "server/protocols/enable";
const SERVER_CONTROL_REQUEST_ID: u64 = 1;

/// Outcome of [`acquire_singleton_role`]: either this process runs the server
/// or it should connect to an already-running instance using the metadata.
#[derive(Debug)]
pub(crate) enum SingletonRole {
    /// Exclusive lock acquired; caller must bind internal proxy and
    /// [`RealServerGuard::publish_endpoint`].
    Real(RealServerGuard),
    /// Another process holds the lock; use `endpoint` + `token` to connect.
    Proxy(ServerLockMetadata),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ServerLifetime {
    #[default]
    Client,
    Daemon,
}

/// Published in `server.lock.json` while the real server is running.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ServerLockMetadata {
    pub(crate) version: u32,
    pub(crate) pid: u32,
    /// Loopback WebSocket URL of the real server's internal proxy listener.
    pub(crate) endpoint: String,
    /// Must be sent as the first WebSocket text frame after connect.
    pub(crate) token: String,
    pub(crate) started_at: String,
    #[serde(default)]
    pub(crate) lifetime: ServerLifetime,
}

impl std::fmt::Debug for ServerLockMetadata {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServerLockMetadata")
            .field("version", &self.version)
            .field("pid", &self.pid)
            .field("endpoint", &self.endpoint)
            .field("token", &"[redacted]")
            .field("started_at", &self.started_at)
            .field("lifetime", &self.lifetime)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ServerControlAction {
    Status,
    Shutdown,
    ShutdownDaemon,
    EnableProtocols(ProtocolSet),
}

impl ServerControlAction {
    pub(crate) fn method(&self) -> &'static str {
        match self {
            ServerControlAction::Status => SERVER_CONTROL_STATUS_METHOD,
            ServerControlAction::Shutdown => SERVER_CONTROL_SHUTDOWN_METHOD,
            ServerControlAction::ShutdownDaemon => DAEMON_CONTROL_SHUTDOWN_METHOD,
            ServerControlAction::EnableProtocols(_) => SERVER_CONTROL_ENABLE_PROTOCOLS_METHOD,
        }
    }

    fn params(&self) -> Option<serde_json::Value> {
        match self {
            Self::Status | Self::Shutdown | Self::ShutdownDaemon => None,
            Self::EnableProtocols(protocols) => Some(serde_json::json!({
                "protocols": protocols.names(),
            })),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServerControlResult {
    pub(crate) status: String,
    pub(crate) lifetime: ServerLifetime,
    pub(crate) enabled_protocols: Option<ProtocolSet>,
}

impl ServerLockMetadata {
    fn new(endpoint: String, token: String) -> Self {
        Self {
            version: METADATA_VERSION,
            pid: std::process::id(),
            endpoint,
            token,
            started_at: Utc::now().to_rfc3339(),
            lifetime: ServerLifetime::Client,
        }
    }
}

/// One-shot control RPC against the real server's internal proxy
/// (`devo server --status` / `--shutdown` from a proxy process).
pub(crate) async fn run_server_control(
    metadata: &ServerLockMetadata,
    action: ServerControlAction,
) -> Result<ServerControlResult> {
    tokio::time::timeout(
        Duration::from_secs(5),
        run_server_control_inner(metadata, action),
    )
    .await
    .context("singleton control request timed out")?
}

async fn run_server_control_inner(
    metadata: &ServerLockMetadata,
    action: ServerControlAction,
) -> Result<ServerControlResult> {
    let (socket, _) = connect_async(metadata.endpoint.as_str())
        .await
        .with_context(|| format!("connect to singleton server {}", metadata.endpoint))?;
    let (mut writer, mut reader) = socket.split();
    // Internal proxy requires token auth before any other traffic.
    writer
        .send(Message::Text(metadata.token.clone().into()))
        .await
        .context("authenticate singleton server control request")?;

    let mut request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": SERVER_CONTROL_REQUEST_ID,
        "method": action.method(),
    });
    if let Some(params) = action.params() {
        request["params"] = params;
    }
    writer
        .send(Message::Text(request.to_string().into()))
        .await
        .context("send singleton server control request")?;

    match reader.next().await {
        Some(Ok(Message::Text(text))) => decode_server_control_response(&text),
        Some(Ok(Message::Close(_))) | None => {
            bail!("singleton server closed before answering control request")
        }
        Some(Ok(_)) => bail!("singleton server returned non-text control response"),
        Some(Err(error)) => Err(error).context("read singleton server control response"),
    }
}

/// Holds the exclusive singleton lock until dropped; releasing the lock allows
/// another process to become the real server.
#[derive(Debug)]
pub(crate) struct RealServerGuard {
    lock_file: File,
    metadata_path: PathBuf,
}

impl RealServerGuard {
    /// Writes `server.lock.json` so proxy/control clients can find this server.
    pub(crate) fn publish_endpoint(
        &self,
        endpoint: String,
        lifetime: ServerLifetime,
    ) -> Result<ServerLockMetadata> {
        let mut metadata = ServerLockMetadata::new(endpoint, Uuid::new_v4().to_string());
        metadata.lifetime = lifetime;
        let encoded = serde_json::to_vec_pretty(&metadata).context("serialize server metadata")?;
        crate::singleton_files::publish_metadata(&self.metadata_path, &encoded).with_context(
            || {
                format!(
                    "write server singleton metadata {}",
                    self.metadata_path.display()
                )
            },
        )?;
        Ok(metadata)
    }
}

impl Drop for RealServerGuard {
    fn drop(&mut self) {
        // Best-effort cleanup so a crashed predecessor does not block forever
        // once the OS releases the file handle; metadata removal signals "not running".
        let _ = fs::remove_file(&self.metadata_path);
        let _ = self.lock_file.unlock();
    }
}

/// Attempts an exclusive lock on `DEVO_HOME/server.lock`.
///
/// - Success → [`SingletonRole::Real`]: caller becomes the sole server process.
/// - Lock held by another process → [`SingletonRole::Proxy`]: read metadata and
///   connect instead of starting a second runtime.
pub(crate) fn acquire_singleton_role(devo_home: &Path) -> Result<SingletonRole> {
    fs::create_dir_all(devo_home)
        .with_context(|| format!("create DEVO_HOME {}", devo_home.display()))?;
    let lock_path = lock_path(devo_home);
    let lock_file = crate::singleton_files::open_lock(&lock_path)
        .with_context(|| format!("open server singleton lock {}", lock_path.display()))?;

    for _ in 0..METADATA_READ_RETRIES {
        match lock_file.try_lock_exclusive() {
            Ok(()) => {
                let _ = fs::remove_file(metadata_path(devo_home));
                return Ok(SingletonRole::Real(RealServerGuard {
                    lock_file,
                    metadata_path: metadata_path(devo_home),
                }));
            }
            Err(error) if is_lock_temporarily_unavailable(&error) => {
                if let Ok(metadata) = read_metadata(devo_home) {
                    return Ok(SingletonRole::Proxy(metadata));
                }
                std::thread::sleep(METADATA_READ_RETRY_DELAY);
            }
            Err(error) => return Err(error).context("acquire singleton owner lock"),
        }
    }
    bail!("singleton owner has not published valid metadata; retry after startup completes")
}

/// A status probe never publishes/removes metadata or keeps an owner guard.
pub(crate) fn probe_singleton(home: &Path) -> Result<Option<ServerLockMetadata>> {
    if !owner_lock_held(home)? {
        return Ok(None);
    }
    read_metadata(home).map(Some)
}

pub(crate) fn owner_lock_held(home: &Path) -> Result<bool> {
    let file = crate::singleton_files::open_lock(&lock_path(home))?;
    match file.try_lock_exclusive() {
        Ok(()) => {
            file.unlock()?;
            Ok(false)
        }
        Err(error) if is_lock_temporarily_unavailable(&error) => Ok(true),
        Err(error) => Err(error).context("probe singleton owner lock"),
    }
}

/// Lightweight stdio front-end for an already-running real server.
///
/// Connects to the internal proxy WebSocket, authenticates with `token`, then:
/// - stdin lines (NDJSON) → WebSocket Text frames upstream
/// - WebSocket Text frames → stdout (one line per frame, NDJSON framing preserved)
///
/// On the real server this connection is registered as `ClientTransportKind::StdioProxy`.
pub(crate) async fn run_stdio_proxy(metadata: ServerLockMetadata) -> Result<()> {
    let (socket, _) = connect_async(metadata.endpoint.as_str())
        .await
        .with_context(|| format!("connect to singleton server {}", metadata.endpoint))?;
    let (mut writer, mut reader) = socket.split();
    writer
        .send(Message::Text(metadata.token.into()))
        .await
        .context("authenticate singleton stdio proxy")?;

    let mut stdin_task = tokio::spawn(async move {
        let stdin = tokio::io::stdin();
        let mut lines = BufReader::new(stdin).lines();
        while let Some(line) = lines.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }
            writer
                .send(Message::Text(line.into()))
                .await
                .context("forward stdio request to singleton server")?;
        }
        let _ = writer.send(Message::Close(None)).await;
        Result::<()>::Ok(())
    });

    let mut stdout_task = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(frame) = reader.next().await {
            match frame.context("read singleton server frame")? {
                Message::Text(text) => {
                    stdout
                        .write_all(text.as_bytes())
                        .await
                        .context("write singleton response to stdout")?;
                    // Real server sends one JSON-RPC message per WebSocket frame;
                    // re-add the newline stdio clients expect.
                    stdout
                        .write_all(b"\n")
                        .await
                        .context("write singleton response newline")?;
                    stdout
                        .flush()
                        .await
                        .context("flush singleton response to stdout")?;
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        Result::<()>::Ok(())
    });

    tokio::select! {
        result = &mut stdin_task => {
            stdout_task.abort();
            result.context("join singleton proxy stdin task")??;
        }
        result = &mut stdout_task => {
            stdin_task.abort();
            result.context("join singleton proxy stdout task")??;
        }
    }
    Ok(())
}

fn decode_server_control_response(text: &str) -> Result<ServerControlResult> {
    let value: serde_json::Value =
        serde_json::from_str(text).context("decode singleton server control response")?;
    if value.get("id") != Some(&serde_json::json!(SERVER_CONTROL_REQUEST_ID)) {
        bail!("singleton server returned response for unexpected control request id");
    }
    if let Some(error) = value.get("error") {
        bail!("singleton server control request failed: {error}");
    }
    let status = value
        .pointer("/result/status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("ok")
        .to_string();
    let enabled_protocols = value
        .pointer("/result/enabledProtocols")
        .map(|value| {
            serde_json::from_value::<Vec<ServerProtocol>>(value.clone())
                .context("decode enabled server protocols")
                .and_then(|protocols| ProtocolSet::new(protocols).map_err(anyhow::Error::msg))
        })
        .transpose()?;
    Ok(ServerControlResult {
        lifetime: value
            .pointer("/result/lifetime")
            .cloned()
            .map(serde_json::from_value)
            .transpose()?
            .unwrap_or_default(),
        status,
        enabled_protocols,
    })
}

fn is_lock_temporarily_unavailable(error: &std::io::Error) -> bool {
    if error.kind() == ErrorKind::WouldBlock {
        return true;
    }

    #[cfg(windows)]
    {
        // Windows may report either sharing or byte-range lock conflicts when
        // another process owns the singleton lock file.
        matches!(error.raw_os_error(), Some(32 | 33))
    }

    #[cfg(not(windows))]
    {
        false
    }
}

fn read_metadata(devo_home: &Path) -> Result<ServerLockMetadata> {
    let path = metadata_path(devo_home);
    let encoded = crate::singleton_files::read_metadata(&path)
        .with_context(|| format!("read server singleton metadata {}", path.display()))?;
    let metadata: ServerLockMetadata =
        serde_json::from_slice(&encoded).context("decode server singleton metadata")?;
    if metadata.version != METADATA_VERSION {
        bail!(
            "unsupported server singleton metadata version {}; expected {METADATA_VERSION}",
            metadata.version
        );
    }
    let uri: tokio_tungstenite::tungstenite::http::Uri = metadata
        .endpoint
        .parse()
        .context("invalid singleton endpoint")?;
    if uri.scheme_str() != Some("ws")
        || !matches!(uri.host(), Some("127.0.0.1" | "[::1]"))
        || uri.port_u16().is_none()
        || uri.path() != "/"
        || uri.query().is_some()
        || metadata.endpoint.contains('@')
        || metadata.endpoint.contains('#')
    {
        bail!("singleton endpoint must be an authenticated numeric loopback WebSocket");
    }
    Ok(metadata)
}

fn lock_path(devo_home: &Path) -> PathBuf {
    devo_home.join(LOCK_FILE_NAME)
}

fn metadata_path(devo_home: &Path) -> PathBuf {
    devo_home.join(METADATA_FILE_NAME)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn lock_unavailable_detection_handles_would_block() {
        assert_eq!(
            [is_lock_temporarily_unavailable(&std::io::Error::new(
                ErrorKind::WouldBlock,
                "lock held",
            ))],
            [true]
        );
    }

    #[test]
    fn server_control_action_methods_match_internal_methods() {
        assert_eq!(
            [
                ServerControlAction::Status.method(),
                ServerControlAction::Shutdown.method(),
                ServerControlAction::EnableProtocols(ProtocolSet::only(ServerProtocol::Acp))
                    .method(),
            ],
            [
                SERVER_CONTROL_STATUS_METHOD,
                SERVER_CONTROL_SHUTDOWN_METHOD,
                SERVER_CONTROL_ENABLE_PROTOCOLS_METHOD,
            ]
        );
    }

    #[test]
    fn decode_server_control_response_reads_status() {
        assert_eq!(
            decode_server_control_response(
                r#"{"jsonrpc":"2.0","id":1,"result":{"status":"running"}}"#
            )
            .expect("decode response"),
            ServerControlResult {
                status: "running".to_string(),
                lifetime: ServerLifetime::Client,
                enabled_protocols: None,
            }
        );
    }

    #[test]
    fn decode_server_control_response_reads_enabled_protocols() {
        assert_eq!(
            decode_server_control_response(
                r#"{"jsonrpc":"2.0","id":1,"result":{"status":"running","enabledProtocols":["native","acp"]}}"#
            )
            .expect("decode response"),
            ServerControlResult {
                status: "running".to_string(),
                lifetime: ServerLifetime::Client,
                enabled_protocols: Some(ProtocolSet::all()),
            }
        );
    }

    #[cfg(windows)]
    #[test]
    fn lock_unavailable_detection_handles_windows_lock_errors() {
        assert_eq!(
            [32, 33]
                .into_iter()
                .map(|code| {
                    is_lock_temporarily_unavailable(&std::io::Error::from_raw_os_error(code))
                })
                .collect::<Vec<_>>(),
            vec![true, true]
        );
    }

    #[test]
    fn real_guard_publishes_metadata_without_working_root() {
        let temp_dir = TempDir::new().expect("temp dir");
        let role = acquire_singleton_role(temp_dir.path()).expect("singleton role");
        let SingletonRole::Real(guard) = role else {
            panic!("expected real server role");
        };

        let metadata = guard
            .publish_endpoint("ws://127.0.0.1:0".to_string(), ServerLifetime::Client)
            .expect("publish endpoint");
        let encoded = fs::read_to_string(metadata_path(temp_dir.path())).expect("metadata file");
        let persisted: ServerLockMetadata = serde_json::from_str(&encoded).expect("metadata json");

        assert_eq!(persisted, metadata);
        assert!(!encoded.contains("working_root"));
    }

    #[test]
    fn metadata_only_accepts_numeric_loopback_and_redacts_debug_token() {
        let home = TempDir::new().unwrap();
        for endpoint in ["ws://127.0.0.1:3210", "ws://[::1]:3210/"] {
            let metadata = ServerLockMetadata::new(endpoint.into(), "secret-test-token".into());
            crate::singleton_files::publish_metadata(
                &metadata_path(home.path()),
                &serde_json::to_vec(&metadata).unwrap(),
            )
            .unwrap();
            assert_eq!(read_metadata(home.path()).unwrap(), metadata);
            assert!(!format!("{metadata:?}").contains("secret-test-token"));
        }
        for endpoint in [
            "ws://example.com:3210",
            "ws://localhost:3210",
            "wss://127.0.0.1:3210",
            "ws://127.0.0.1:3210/other",
            "ws://127.0.0.1:3210/?query",
            "ws://token@127.0.0.1:3210/",
            "ws://127.0.0.1:3210/#fragment",
        ] {
            let metadata = ServerLockMetadata::new(endpoint.into(), "secret-test-token".into());
            crate::singleton_files::publish_metadata(
                &metadata_path(home.path()),
                &serde_json::to_vec(&metadata).unwrap(),
            )
            .unwrap();
            assert!(read_metadata(home.path()).is_err(), "{endpoint}");
        }
    }

    #[test]
    fn status_probe_preserves_metadata_and_never_keeps_ownership() {
        let home = TempDir::new().unwrap();
        let SingletonRole::Real(guard) = acquire_singleton_role(home.path()).unwrap() else {
            panic!("owner");
        };
        let metadata = guard
            .publish_endpoint("ws://127.0.0.1:3210".into(), ServerLifetime::Daemon)
            .unwrap();
        assert_eq!(
            probe_singleton(home.path()).unwrap(),
            Some(metadata.clone())
        );
        drop(guard);
        // Simulate a crash residue: an absent owner must not remove it on status.
        crate::singleton_files::publish_metadata(
            &metadata_path(home.path()),
            &serde_json::to_vec(&metadata).unwrap(),
        )
        .unwrap();
        assert_eq!(probe_singleton(home.path()).unwrap(), None);
        assert!(metadata_path(home.path()).exists());
        assert!(matches!(
            acquire_singleton_role(home.path()).unwrap(),
            SingletonRole::Real(_)
        ));
    }

    #[test]
    fn dropping_real_guard_removes_metadata() {
        let temp_dir = TempDir::new().expect("temp dir");
        let role = acquire_singleton_role(temp_dir.path()).expect("singleton role");
        let SingletonRole::Real(guard) = role else {
            panic!("expected real server role");
        };
        guard
            .publish_endpoint("ws://127.0.0.1:0".to_string(), ServerLifetime::Client)
            .expect("publish endpoint");
        let metadata_path = metadata_path(temp_dir.path());

        drop(guard);

        assert!(!metadata_path.exists());
    }
}

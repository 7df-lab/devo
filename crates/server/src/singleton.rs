use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
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

const LOCK_FILE_NAME: &str = "server.lock";
const METADATA_FILE_NAME: &str = "server.lock.json";
const METADATA_VERSION: u32 = 1;
const METADATA_READ_RETRIES: usize = 100;
const METADATA_READ_RETRY_DELAY: Duration = Duration::from_millis(50);

#[derive(Debug)]
pub(crate) enum SingletonRole {
    Real(RealServerGuard),
    Proxy(ServerLockMetadata),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ServerLockMetadata {
    pub(crate) version: u32,
    pub(crate) pid: u32,
    pub(crate) endpoint: String,
    pub(crate) token: String,
    pub(crate) started_at: String,
}

impl ServerLockMetadata {
    fn new(endpoint: String, token: String) -> Self {
        Self {
            version: METADATA_VERSION,
            pid: std::process::id(),
            endpoint,
            token,
            started_at: Utc::now().to_rfc3339(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct RealServerGuard {
    lock_file: File,
    metadata_path: PathBuf,
}

impl RealServerGuard {
    pub(crate) fn publish_endpoint(&self, endpoint: String) -> Result<ServerLockMetadata> {
        let metadata = ServerLockMetadata::new(endpoint, Uuid::new_v4().to_string());
        let encoded = serde_json::to_vec_pretty(&metadata).context("serialize server metadata")?;
        fs::write(&self.metadata_path, encoded).with_context(|| {
            format!(
                "write server singleton metadata {}",
                self.metadata_path.display()
            )
        })?;
        Ok(metadata)
    }
}

impl Drop for RealServerGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.metadata_path);
        let _ = self.lock_file.unlock();
    }
}

pub(crate) fn acquire_singleton_role(devo_home: &Path) -> Result<SingletonRole> {
    fs::create_dir_all(devo_home)
        .with_context(|| format!("create DEVO_HOME {}", devo_home.display()))?;
    let lock_path = lock_path(devo_home);
    let lock_file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("open server singleton lock {}", lock_path.display()))?;

    match lock_file.try_lock_exclusive() {
        Ok(()) => Ok(SingletonRole::Real(RealServerGuard {
            lock_file,
            metadata_path: metadata_path(devo_home),
        })),
        Err(error) if error.kind() == ErrorKind::WouldBlock => {
            Ok(SingletonRole::Proxy(read_metadata_with_retry(devo_home)?))
        }
        Err(error) => {
            Err(error).with_context(|| format!("lock server singleton {}", lock_path.display()))
        }
    }
}

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

fn read_metadata_with_retry(devo_home: &Path) -> Result<ServerLockMetadata> {
    let mut last_error = None;
    for _ in 0..METADATA_READ_RETRIES {
        match read_metadata(devo_home) {
            Ok(metadata) => return Ok(metadata),
            Err(error) => {
                last_error = Some(error);
                std::thread::sleep(METADATA_READ_RETRY_DELAY);
            }
        }
    }
    Err(last_error.expect("metadata read should have failed at least once"))
}

fn read_metadata(devo_home: &Path) -> Result<ServerLockMetadata> {
    let path = metadata_path(devo_home);
    let encoded = fs::read(&path)
        .with_context(|| format!("read server singleton metadata {}", path.display()))?;
    let metadata: ServerLockMetadata =
        serde_json::from_slice(&encoded).context("decode server singleton metadata")?;
    if metadata.version != METADATA_VERSION {
        bail!(
            "unsupported server singleton metadata version {}; expected {METADATA_VERSION}",
            metadata.version
        );
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
    fn real_guard_publishes_metadata_without_working_root() {
        let temp_dir = TempDir::new().expect("temp dir");
        let role = acquire_singleton_role(temp_dir.path()).expect("singleton role");
        let SingletonRole::Real(guard) = role else {
            panic!("expected real server role");
        };

        let metadata = guard
            .publish_endpoint("ws://127.0.0.1:0".to_string())
            .expect("publish endpoint");
        let encoded = fs::read_to_string(metadata_path(temp_dir.path())).expect("metadata file");
        let persisted: ServerLockMetadata = serde_json::from_str(&encoded).expect("metadata json");

        assert_eq!(persisted, metadata);
        assert!(!encoded.contains("working_root"));
    }

    #[test]
    fn dropping_real_guard_removes_metadata() {
        let temp_dir = TempDir::new().expect("temp dir");
        let role = acquire_singleton_role(temp_dir.path()).expect("singleton role");
        let SingletonRole::Real(guard) = role else {
            panic!("expected real server role");
        };
        guard
            .publish_endpoint("ws://127.0.0.1:0".to_string())
            .expect("publish endpoint");
        let metadata_path = metadata_path(temp_dir.path());

        drop(guard);

        assert!(!metadata_path.exists());
    }
}

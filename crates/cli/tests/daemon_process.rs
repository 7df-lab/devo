//! Real process ownership/attach/recovery contracts; no provider credentials.
#![cfg(unix)]
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use futures::{SinkExt, StreamExt};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

fn command(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_devo"));
    command
        .env("DEVO_HOME", home)
        .env("TOKIO_WORKER_THREADS", "2")
        .env_remove("OPENAI_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .kill_on_drop(true);
    command
}

async fn run(home: &Path, args: &[&str]) -> Result<std::process::Output> {
    tokio::time::timeout(Duration::from_secs(60), command(home).args(args).output())
        .await
        .context("command deadline")?
        .context("run command")
}

async fn ready(home: &Path) -> Result<devo_server::DaemonStatus> {
    tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            if let Ok(Some(status)) = devo_server::daemon_status(home).await {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .context("daemon readiness")
}

struct Attached {
    child: Child,
    input: ChildStdin,
    output: Lines<BufReader<ChildStdout>>,
    next_id: u64,
}

impl Attached {
    async fn open(home: &Path) -> Result<Self> {
        let mut child = command(home)
            .args(["daemon", "attach"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let input = child.stdin.take().context("stdin")?;
        let output = BufReader::new(child.stdout.take().context("stdout")?).lines();
        let mut attached = Self {
            child,
            input,
            output,
            next_id: 0,
        };
        attached
            .request(
                "initialize",
                json!({
                    "protocolVersion": 1, "clientCapabilities": {"terminal": false},
                    "_meta": { "devo": { "protocol": "native" } },
                }),
            )
            .await?;
        Ok(attached)
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        self.input
            .write_all(
                format!(
                    "{}\n",
                    json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params})
                )
                .as_bytes(),
            )
            .await?;
        self.input.flush().await?;
        tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(line) = self.output.next_line().await? {
                let value: Value = serde_json::from_str(&line)?;
                if value["id"] == id {
                    anyhow::ensure!(value.get("error").is_none(), "RPC failed: {value}");
                    return Ok(value["result"].clone());
                }
            }
            anyhow::bail!("attach closed before response")
        })
        .await
        .context("RPC deadline")?
    }

    async fn disconnect(mut self) -> Result<()> {
        drop(self.input);
        tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await??;
        Ok(())
    }
}

fn home() -> Result<tempfile::TempDir> {
    let home = tempfile::tempdir()?;
    std::fs::write(
        home.path().join("config.toml"),
        "[catalog]\nrefresh_on_startup = false\n[server]\nlisten = [\"invalid-public-listener\"]\n",
    )?;
    Ok(home)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_survives_attach_disconnect_and_refuses_legacy_shutdown() -> Result<()> {
    let home = home()?;
    let absent = run(home.path(), &["daemon", "attach"]).await?;
    assert!(!absent.status.success());
    assert!(!home.path().join("server.lock.json").exists());
    let mut owner = command(home.path())
        .args(["daemon", "run"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let status = ready(home.path()).await?;
    let mut client = Attached::open(home.path()).await?;
    client.request("runtime/ping", json!({})).await?;
    client.disconnect().await?;
    assert_eq!(ready(home.path()).await?, status);
    let legacy = run(home.path(), &["server", "--shutdown"]).await?;
    assert!(!legacy.status.success());
    assert_eq!(ready(home.path()).await?, status);
    // Old Desktop bundles issue this wire method without the new CLI guard.
    let metadata: Value =
        serde_json::from_slice(&std::fs::read(home.path().join("server.lock.json"))?)?;
    let (mut socket, _) =
        tokio_tungstenite::connect_async(metadata["endpoint"].as_str().context("endpoint")?)
            .await?;
    use tokio_tungstenite::tungstenite::Message;
    socket
        .send(Message::Text(
            metadata["token"]
                .as_str()
                .context("token")?
                .to_string()
                .into(),
        ))
        .await?;
    socket
        .send(Message::Text(
            json!({"jsonrpc":"2.0","id":99,"method":"server/shutdown"})
                .to_string()
                .into(),
        ))
        .await?;
    let response = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await?
        .context("shutdown response")??;
    let response: Value = serde_json::from_str(response.to_text()?)?;
    assert!(response.get("error").is_some());
    assert_eq!(ready(home.path()).await?, status);
    let mut attached = Vec::new();
    for _ in 0..16 {
        attached.push(Attached::open(home.path()).await?);
    }
    // Full long-lived data-client capacity still leaves status/stop slots.
    assert_eq!(ready(home.path()).await?, status);
    assert!(Attached::open(home.path()).await.is_err());
    devo_server::stop_daemon(home.path()).await?;
    assert!(
        attached[0]
            .request("runtime/ping", json!({}))
            .await
            .is_err()
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(10), owner.wait())
            .await??
            .success()
    );
    assert_eq!(devo_server::daemon_status(home.path()).await?, None);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crash_restart_rebuilds_index_before_claiming_persisted_schedule() -> Result<()> {
    let home = home()?;
    let mut owner = command(home.path())
        .args(["daemon", "run"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    ready(home.path()).await?;
    let mut client = Attached::open(home.path()).await?;
    let session = client
        .request(
            "session/new",
            json!({"cwd":home.path(), "idempotencyKey":"daemon-fixture"}),
        )
        .await?;
    let session_id = session["session"]["id"].clone();
    let job = client
        .request(
            "session/schedule/upsert",
            json!({
                "kind":"cron", "sessionId":session_id, "intervalMs":3_600_000,
                "prompt":"isolated recovery fixture", "deliveryMode":"followUp",
            }),
        )
        .await?;
    let job_id = job["job"]["jobId"].clone();
    client.disconnect().await?;
    // Force process loss, leaving a stale metadata file and durable journal.
    owner.kill().await?;
    let schedules = home.path().join("schedules.json");
    let mut persisted: Value = serde_json::from_slice(&std::fs::read(&schedules)?)?;
    persisted["jobs"][0]["nextRunAt"] = json!("2026-01-01T00:00:00Z");
    std::fs::write(&schedules, serde_json::to_vec(&persisted)?)?;
    for name in ["devo.db", "devo.db-wal", "devo.db-shm"] {
        let _ = std::fs::remove_file(home.path().join(name));
    }
    let mut owner = command(home.path())
        .args(["daemon", "run"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    ready(home.path()).await?;
    let mut client = Attached::open(home.path()).await?;
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let jobs = client
                .request("session/schedule/list", json!({"sessionId":session_id}))
                .await?;
            assert_eq!(jobs["jobs"].as_array().context("jobs")?.len(), 1);
            assert_eq!(jobs["jobs"][0]["jobId"], job_id);
            assert_eq!(jobs["jobs"][0]["status"], "active");
            if jobs["jobs"][0]["runCount"].as_u64().context("run count")? >= 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Ok::<(), anyhow::Error>(())
    })
    .await??;
    client.disconnect().await?;
    let scheduled_at = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")?;
    let occurrence_id = format!(
        "schedule:{}:{}",
        job_id.as_str().context("job id")?,
        scheduled_at.timestamp_micros()
    );
    let rollout = home.path().join("sessions").join(format!(
        "{}.jsonl",
        session_id.as_str().context("session id")?
    ));
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if materialized_count(&rollout, &occurrence_id) == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .context("schedule must materialize without an attached client")?;
    devo_server::stop_daemon(home.path()).await?;
    assert!(
        tokio::time::timeout(Duration::from_secs(10), owner.wait())
            .await??
            .success()
    );
    let mut restarted = command(home.path())
        .args(["daemon", "run"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    ready(home.path()).await?;
    // Observe reconciliation, rather than stopping before the first tick.
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let outbox: Value =
                serde_json::from_slice(&std::fs::read(home.path().join("schedule-outbox.json"))?)?;
            let pending = outbox["occurrences"]
                .as_array()
                .context("occurrences")?
                .iter()
                .any(|item| item["occurrence_id"] == occurrence_id);
            if !pending {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Ok::<(), anyhow::Error>(())
    })
    .await
    .context("restarted dispatcher must acknowledge the materialized occurrence")??;
    devo_server::stop_daemon(home.path()).await?;
    assert!(
        tokio::time::timeout(Duration::from_secs(10), restarted.wait())
            .await??
            .success()
    );
    assert_eq!(materialized_count(&rollout, &occurrence_id), 1);
    Ok(())
}

fn materialized_count(rollout: &Path, occurrence_id: &str) -> usize {
    devo_core::read_canonical_history(rollout).map(|history| history.items.iter().filter(|envelope| {
        matches!(&envelope.item, devo_protocol::native::item::Item::UserMessage { client_user_message_id: Some(id), .. } if id == occurrence_id)
    }).count()).unwrap_or(0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detached_start_is_idempotent_and_remains_live_after_launchers_exit() -> Result<()> {
    let home = home()?;
    let result = async {
        let (first, second) = tokio::join!(
            run(home.path(), &["daemon", "start"]),
            run(home.path(), &["daemon", "start"]),
        );
        let first = first?;
        let second = second?;
        anyhow::ensure!(
            first.status.success(),
            "first start: {}",
            String::from_utf8_lossy(&first.stderr)
        );
        anyhow::ensure!(
            second.status.success(),
            "second start: {}",
            String::from_utf8_lossy(&second.stderr)
        );
        let status = ready(home.path()).await?;
        let mut client = Attached::open(home.path()).await?;
        client.request("runtime/ping", json!({})).await?;
        client.disconnect().await?;
        assert_eq!(ready(home.path()).await?, status);
        Ok::<(), anyhow::Error>(())
    }
    .await;
    let cleanup = devo_server::stop_daemon(home.path()).await;
    result?;
    cleanup?;
    assert_eq!(devo_server::daemon_status(home.path()).await?, None);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn supervisor_sigterm_releases_daemon_ownership() -> Result<()> {
    let home = home()?;
    let mut owner = command(home.path())
        .args(["daemon", "run"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    ready(home.path()).await?;
    // Signal only this test's still-owned child, never a PID from metadata.
    let pid = i32::try_from(owner.id().context("child id")?)?;
    assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
    assert!(
        tokio::time::timeout(Duration::from_secs(10), owner.wait())
            .await??
            .success()
    );
    assert_eq!(devo_server::daemon_status(home.path()).await?, None);
    assert!(!home.path().join("server.lock.json").exists());
    Ok(())
}

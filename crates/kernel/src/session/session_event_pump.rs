//! Single-owner kernel stdout pump.
//!
//! The pump remains alive between cells so late Bash completion requests can
//! reach a narrowly scoped idle handler. Cell events are demultiplexed to the
//! current request; host requests are handled here and never use a stale turn
//! callback after that request's `done` event.

use std::sync::Arc;

use tokio::io::BufReader;
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::{Mutex, mpsc};

use crate::protocol::{KernelEvent, KernelRequest};

use super::{HostRequestHandler, deny_host_handler};

pub(super) const EVENT_CHANNEL_CAPACITY: usize = 128;

#[derive(Clone)]
pub(super) struct ActiveRequestContext {
    pub id: String,
    pub host_handler: Option<HostRequestHandler>,
}

pub(super) fn spawn(
    mut stdout: BufReader<ChildStdout>,
    stdin: Arc<Mutex<ChildStdin>>,
    active_request: Arc<Mutex<Option<ActiveRequestContext>>>,
    idle_bash_handler: Arc<Mutex<Option<HostRequestHandler>>>,
    event_tx: mpsc::Sender<Result<KernelEvent, super::ReplError>>,
) {
    tokio::spawn(async move {
        loop {
            let event = match super::read_line_event(&mut stdout).await {
                Ok(event) => event,
                Err(error) => {
                    let _ = event_tx.send(Err(error)).await;
                    break;
                }
            };
            match event {
                KernelEvent::HostRequest { id, cell_id, data } => {
                    let active_handler = active_request
                        .lock()
                        .await
                        .as_ref()
                        .filter(|request| cell_id.as_deref() == Some(request.id.as_str()))
                        .and_then(|request| request.host_handler.clone());
                    let action = data
                        .get("type")
                        .or_else(|| data.get("action"))
                        .and_then(serde_json::Value::as_str);
                    let handler = if let Some(handler) = active_handler {
                        handler
                    } else if matches!(action, Some("bash.completed" | "bash.consumed")) {
                        idle_bash_handler
                            .lock()
                            .await
                            .clone()
                            .unwrap_or_else(deny_host_handler)
                    } else {
                        deny_host_handler()
                    };
                    let reply_data = handler(id.clone(), data).await;
                    let reply = KernelRequest::HostReply {
                        id,
                        data: reply_data,
                    };
                    let mut stdin = stdin.lock().await;
                    if let Err(error) = super::write_request(&mut stdin, &reply).await {
                        let _ = event_tx.send(Err(error)).await;
                        break;
                    }
                }
                KernelEvent::Done {
                    id,
                    status,
                    names,
                    reason,
                    extra,
                } => {
                    let is_current = {
                        let mut active_request = active_request.lock().await;
                        if active_request
                            .as_ref()
                            .is_some_and(|request| request.id == id)
                        {
                            active_request.take();
                            true
                        } else {
                            false
                        }
                    };
                    if is_current
                        && event_tx
                            .send(Ok(KernelEvent::Done {
                                id,
                                status,
                                names,
                                reason,
                                extra,
                            }))
                            .await
                            .is_err()
                    {
                        break;
                    }
                }
                other => {
                    let should_forward = active_request
                        .lock()
                        .await
                        .as_ref()
                        .is_some_and(|request| event_matches_request(&other, &request.id));
                    if should_forward && event_tx.send(Ok(other)).await.is_err() {
                        break;
                    }
                }
            }
        }
    });
}

fn event_matches_request(event: &KernelEvent, request_id: &str) -> bool {
    match event {
        KernelEvent::Stdout { id, .. }
        | KernelEvent::Stderr { id, .. }
        | KernelEvent::Error { id, .. }
        | KernelEvent::Display { id, .. } => {
            id.as_deref().is_none_or(|event_id| event_id == request_id)
        }
        KernelEvent::Result { id, .. } | KernelEvent::Done { id, .. } => id == request_id,
        KernelEvent::Ready { .. } | KernelEvent::HostRequest { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use std::process::Stdio;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use serde_json::{Value, json};
    use tokio::process::Command;
    use tokio::sync::Notify;

    use super::super::{HostRequestHandler, KernelSession};

    #[tokio::test]
    async fn idle_host_request_wakes_bash_completion_without_stealing_next_cell() {
        let script = r#"
import json, sys
print(json.dumps({"event":"ready","protocol":3,"python":"fake"}), flush=True)
for line in sys.stdin:
    req = json.loads(line)
    if req.get("type") == "execute":
        cell_id = req["id"]
        if req.get("code") == "first":
            print(json.dumps({"event":"host_request","id":"h-active","cell_id":cell_id,"data":{"type":"active.test"}}), flush=True)
            active_reply = json.loads(sys.stdin.readline())
            assert active_reply["type"] == "host_reply", active_reply
            print(json.dumps({"event":"stdout","id":cell_id,"text":active_reply["data"]["result"]["scope"]+"\n"}), flush=True)
            print(json.dumps({"event":"done","id":cell_id,"status":"ok"}), flush=True)
            print(json.dumps({"event":"stdout","id":cell_id,"text":"stale output after done\n"}), flush=True)
            print(json.dumps({"event":"done","id":"foreign-id","status":"error"}), flush=True)
            print(json.dumps({"event":"host_request","id":"h-stale","cell_id":cell_id,"data":{"type":"unsafe.late"}}), flush=True)
            stale_reply = json.loads(sys.stdin.readline())
            assert stale_reply["data"]["status"] == "error", stale_reply
            print(json.dumps({"event":"host_request","id":"h-bash","cell_id":cell_id,"data":{"type":"bash.completed","params":{"pid":77,"exitCode":0}}}), flush=True)
            bash_reply = json.loads(sys.stdin.readline())
            assert bash_reply["data"]["result"]["accepted"] is True, bash_reply
            print(json.dumps({"event":"host_request","id":"h-consumed","cell_id":cell_id,"data":{"type":"bash.consumed","params":{"pid":77}}}), flush=True)
            consumed_reply = json.loads(sys.stdin.readline())
            assert consumed_reply["data"]["result"]["accepted"] is True, consumed_reply
        else:
            print(json.dumps({"event":"stdout","id":"foreign-id","text":"foreign output\n"}), flush=True)
            print(json.dumps({"event":"done","id":"foreign-id","status":"error"}), flush=True)
            print(json.dumps({"event":"stdout","id":cell_id,"text":"next cell intact\n"}), flush=True)
            print(json.dumps({"event":"done","id":cell_id,"status":"ok"}), flush=True)
    elif req.get("type") == "shutdown":
        print(json.dumps({"event":"done","id":req.get("id") or "","status":"ok"}), flush=True)
        break
"#;
        let dir = tempfile::tempdir().expect("tempdir");
        let script_path = dir.path().join("idle_host_repl.py");
        std::fs::write(&script_path, script).expect("write fake repl");
        let mut command = Command::new("python");
        command
            .arg(&script_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());

        let active_calls = Arc::new(AtomicUsize::new(0));
        let active_calls_handler = Arc::clone(&active_calls);
        let active_handler: HostRequestHandler = Arc::new(move |_id, _data| {
            let active_calls = Arc::clone(&active_calls_handler);
            Box::pin(async move {
                active_calls.fetch_add(1, Ordering::SeqCst);
                json!({"status":"ok","result":{"scope":"active-cell"}})
            })
        });
        let session = match KernelSession::spawn_command(command, Some(active_handler)).await {
            Ok(session) => session,
            Err(error) => {
                eprintln!("skip: could not spawn fake repl: {error}");
                return;
            }
        };
        let idle_calls = Arc::new(AtomicUsize::new(0));
        let idle_calls_handler = Arc::clone(&idle_calls);
        let idle_notify = Arc::new(Notify::new());
        let idle_notify_handler = Arc::clone(&idle_notify);
        let idle_handler: HostRequestHandler = Arc::new(move |_id, data: Value| {
            let idle_calls = Arc::clone(&idle_calls_handler);
            let idle_notify = Arc::clone(&idle_notify_handler);
            Box::pin(async move {
                if matches!(
                    data.get("type").and_then(Value::as_str),
                    Some("bash.completed" | "bash.consumed")
                ) {
                    idle_calls.fetch_add(1, Ordering::SeqCst);
                    idle_notify.notify_one();
                    json!({"status":"ok","result":{"accepted":true}})
                } else {
                    json!({"status":"error","error":"idle handler only accepts bash.completed"})
                }
            })
        });
        session
            .set_idle_bash_completion_handler(Some(idle_handler))
            .await;

        let first = session.execute("first").await.expect("first cell");
        assert_eq!(first.stdout, "active-cell\n");
        tokio::time::timeout(Duration::from_secs(1), idle_notify.notified())
            .await
            .expect("late Bash completion must wake while idle");
        tokio::time::timeout(Duration::from_secs(1), async {
            while idle_calls.load(Ordering::SeqCst) < 2 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("late consumed notice must reach the restricted idle handler");
        assert_eq!(active_calls.load(Ordering::SeqCst), 1);
        assert_eq!(idle_calls.load(Ordering::SeqCst), 2);

        let second = session.execute("second").await.expect("second cell");
        assert_eq!(second.stdout, "next cell intact\n");
        assert_eq!(second.status, "ok");
    }
    #[tokio::test]
    async fn live_python_host_request_keeps_cell_scope_and_late_bash_uses_idle_handler() {
        use std::path::PathBuf;

        use super::super::KernelSessionConfig;

        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let Some(runtime_path) = super::super::default_runtime_pythonpath(&manifest) else {
            eprintln!("skip: kernel runtime not found");
            return;
        };
        let config = KernelSessionConfig {
            python: PathBuf::from("python"),
            cwd: std::env::temp_dir(),
            python_path_entries: vec![runtime_path],
            extra_env: Vec::new(),
            fence: None,
            workspace_access: crate::WorkspaceAccess::ReadWrite,
        };
        let session = match KernelSession::spawn(config).await {
            Ok(session) => session,
            Err(error) => {
                eprintln!("skip: could not spawn live kernel: {error}");
                return;
            }
        };
        let active_calls = Arc::new(AtomicUsize::new(0));
        let active_calls_handler = Arc::clone(&active_calls);
        session
            .set_host_handler(Some(Arc::new(move |_id, _data| {
                let active_calls = Arc::clone(&active_calls_handler);
                Box::pin(async move {
                    active_calls.fetch_add(1, Ordering::SeqCst);
                    json!({"status":"ok","result":{"scope":"active-cell"}})
                })
            })))
            .await;
        let idle_calls = Arc::new(AtomicUsize::new(0));
        let idle_calls_handler = Arc::clone(&idle_calls);
        let idle_notify = Arc::new(Notify::new());
        let idle_notify_handler = Arc::clone(&idle_notify);
        session
            .set_idle_bash_completion_handler(Some(Arc::new(move |_id, data| {
                let idle_calls = Arc::clone(&idle_calls_handler);
                let idle_notify = Arc::clone(&idle_notify_handler);
                Box::pin(async move {
                    if data.get("type").and_then(Value::as_str) == Some("bash.completed") {
                        idle_calls.fetch_add(1, Ordering::SeqCst);
                        idle_notify.notify_one();
                        json!({"status":"ok","result":{"accepted":true}})
                    } else {
                        json!({"status":"error","error":"unexpected idle action"})
                    }
                })
            })))
            .await;

        let active = session
            .execute(
                "from rlm.repl import host_request\nreply = await host_request({'type': 'active.test'})\nprint(reply['result']['scope'])",
            )
            .await
            .expect("active host request");
        assert_eq!(active.status, "ok");
        assert!(
            active.stdout.contains("active-cell"),
            "stdout={:?}",
            active.stdout
        );
        assert_eq!(active_calls.load(Ordering::SeqCst), 1);

        let detached = session
            .execute(
                "import asyncio\nfrom rlm.repl import host_request\nasync def _late_bash_notice():\n    await asyncio.sleep(0.05)\n    await host_request({'type': 'bash.completed', 'pid': 77, 'exitCode': 0})\n_late_bash_task = asyncio.create_task(_late_bash_notice())\nprint('detached')",
            )
            .await
            .expect("detached completion cell");
        assert_eq!(detached.status, "ok");
        assert!(detached.stdout.contains("detached"));
        tokio::time::timeout(Duration::from_secs(2), idle_notify.notified())
            .await
            .expect("detached Bash completion should reach the idle handler");
        assert_eq!(idle_calls.load(Ordering::SeqCst), 1);
        assert_eq!(active_calls.load(Ordering::SeqCst), 1);

        let after = session
            .execute("print('reader still active')")
            .await
            .expect("next cell");
        assert_eq!(after.status, "ok");
        assert!(after.stdout.contains("reader still active"));
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn real_background_bash_completion_reaches_idle_handler() {
        use std::path::PathBuf;

        use super::super::KernelSessionConfig;

        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let Some(runtime_path) = super::super::default_runtime_pythonpath(&manifest) else {
            eprintln!("skip: kernel runtime not found");
            return;
        };
        let session = match KernelSession::spawn(KernelSessionConfig {
            python: PathBuf::from("python"),
            cwd: std::env::temp_dir(),
            python_path_entries: vec![runtime_path],
            extra_env: Vec::new(),
            fence: None,
            workspace_access: crate::WorkspaceAccess::ReadWrite,
        })
        .await
        {
            Ok(session) => session,
            Err(error) => {
                eprintln!("skip: could not spawn live kernel: {error}");
                return;
            }
        };
        let idle_calls = Arc::new(AtomicUsize::new(0));
        let idle_calls_handler = Arc::clone(&idle_calls);
        let idle_notify = Arc::new(Notify::new());
        let idle_notify_handler = Arc::clone(&idle_notify);
        session
            .set_idle_bash_completion_handler(Some(Arc::new(move |_id, data| {
                let idle_calls = Arc::clone(&idle_calls_handler);
                let idle_notify = Arc::clone(&idle_notify_handler);
                Box::pin(async move {
                    assert_eq!(
                        data.get("type").and_then(Value::as_str),
                        Some("bash.completed")
                    );
                    idle_calls.fetch_add(1, Ordering::SeqCst);
                    idle_notify.notify_one();
                    json!({"status":"ok","result":{"accepted":true}})
                })
            })))
            .await;

        let output = session
            .execute(
                "from rlm.bash import bash\nhandle = bash(\"python -c 'import time; time.sleep(0.1)'\")\nprint(handle.pid)",
            )
            .await
            .expect("launch detached Bash command");
        assert_eq!(output.status, "ok");
        tokio::time::timeout(Duration::from_secs(3), idle_notify.notified())
            .await
            .expect("real Bash completion must reach the idle handler");
        assert_eq!(idle_calls.load(Ordering::SeqCst), 1);

        let after = session
            .execute("print('bash wake preserved reader')")
            .await
            .expect("next cell");
        assert_eq!(after.status, "ok");
        assert!(after.stdout.contains("bash wake preserved reader"));
    }
}

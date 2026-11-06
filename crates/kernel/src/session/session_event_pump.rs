//! Single-owner kernel stdout pump.
//!
//! A [`KernelSession`](super::KernelSession) spawns one Python child and gives
//! exactly one task the job of reading that child's stdout. This module is that
//! task. It is deliberately small, but it owns several invariants that are easy
//! to break elsewhere:
//!
//! - **One reader.** The kernel's stdout is a newline-delimited JSON protocol
//!   channel. Two readers would split frames, so every event in the session
//!   passes through this loop.
//! - **Demultiplexing.** A single cell can emit many `stdout` frames, `display`
//!   frames, one optional `result`, and one terminal `done`. The pump forwards
//!   only events that belong to the request currently allowed to own stdout.
//! - **Host RPC.** `host_request` frames are not cell output. They are answered
//!   here, on a separate task, while the pump keeps reading stdout.
//! - **Lifecycle.** The pump outlives individual cells. A Bash completion or
//!   `bash.poll` can arrive after the cell that started it has already emitted
//!   `done`; the pump routes those frames to the session-scoped idle handler.
//!
//! # Two layers of "stdout"
//!
//! The child's OS stdin/stdout are the *transport*. Requests are JSON lines on
//! fd 0; events are JSON lines on stdout (a private dup of the original fd 1).
//! [`KernelEvent::Stdout`] and [`KernelEvent::Stderr`] are *logical streams
//! carried inside that transport*, not a second pipe:
//!
//! ```text
//! Python user code
//!   print("hi") ──► tagged sys.stdout ──► {"event":"stdout","id":"c1","text":"hi\n"}
//!   os.write(1, b"hi") ──► captured fd 1 pipe ──► {"event":"stdout","id":null,...}
//!                                                        │
//!                                                        ▼
//!                            protocol fd (dup of fd 1): one JSON object per line
//!                                                        │
//!                                                        ▼
//!                                      Rust pump ──► event channel
//! ```
//!
//! The runtime replaces `sys.stdout` and `sys.stderr` before serving requests.
//! A Python-level write is tagged with the writing cell's id at write time and
//! sent straight to the protocol. Raw fd writes (`os.write`, C extensions,
//! subprocesses, `sys.stdout.buffer.write`) are redirected into private pipes
//! and pumped as `id: null` events: the runtime cannot prove which cell wrote
//! those bytes, so it does not attribute them. Neither path can corrupt the
//! JSON framing: a raw `print()` that wrote plain text to fd 1 would interleave
//! with JSON event lines, so user output is always wrapped in an event first.
//!
//! This is why the event enum has `stdout` and `stderr` variants even though
//! the transport itself is stdin/stdout: the variants preserve *which logical
//! stream* produced the text, let the host keep separate `CellOutput.stdout` /
//! `CellOutput.stderr` buffers, and let the UI style diagnostics differently.
//!
//! # Event routing
//!
//! The pump forwards a decoded event only while a request owns stdout:
//!
//! | Event | Forwarded when | Notes |
//! |---|---|---|
//! | `stdout`, `stderr`, `display`, `error` | `id` is `None` or equals the active request id | `None` is session-global output from a raw fd or a user thread |
//! | `result` | `id` equals the active request id | Optional; at most one per cell |
//! | `done` | `id` equals the active request id | Clears the active capability, then is forwarded |
//! | `host_request` | never forwarded | Answered out of band |
//! | `ready` | never forwarded | Consumed by the spawn handshake before the pump starts |
//!
//! Frames for another id, or frames that arrive after `done`, are dropped. A
//! late `done` from an old cell must not clear a newer request's capability.
//!
//! # `result` versus `done`
//!
//! `result` is *data*: the `repr` of a cell's trailing expression, equivalent
//! to Jupyter's `Out[]`. It is optional and appears at most once. `x = 1` and
//! `print("hi")` produce no `result`; `1 + 1` produces `{"event":"result",
//! "id":"c1","text":"2"}` and binds `_ = 2`.
//!
//! `done` is *lifecycle*: exactly one per id'd request, always the last event
//! with that id. It carries the final `status` (`"ok"` or `"error"`) and, for
//! snapshot/restore/list_names, operation-specific fields such as `saved`,
//! `restored`, or `names`. The collector stops only on `done`. An `error`
//! event is not terminal on its own; `done` follows it.
//!
//! A normal cell therefore looks like:
//!
//! ```text
//! stdout("hello\n")  stdout("world\n")  result("42")  done(status="ok")
//! ```
//!
//! The streams are incremental: a frame is not a line. `done` is the fence
//! after the runtime has drained both captured streams.
//!
//! # Host requests and capabilities
//!
//! A `host_request` carries the id of the live cell that owns the request.
//! The pump clones that cell's `HostRequestHandler` only when `cell_id` matches
//! the active request. This is a capability: a detached task that outlives its
//! cell cannot reuse a later turn's handler. The handler runs on its own task
//! so an approval wait never blocks stdout reading; if the future is dropped
//! during teardown, it must release request-local resources.
//!
//! When no cell is active, only the narrow Bash routes (`bash.completed`,
//! `bash.consumed`, `bash.poll`, `bash.kill`, `bash.forget`) may use the idle
//! handler. `bash.start` and every other action are denied: starting a command
//! requires a live cell capability.
//!
//! # Backpressure and teardown
//!
//! The decoded-event channel is bounded by [`EVENT_CHANNEL_CAPACITY`]. If the
//! collector stops draining, the pump stops reading stdout and the kernel
//! eventually blocks on its next protocol write. That is intentional
//! backpressure; memory does not grow without bound. A read error cancels the
//! session lifetime and sends one final `Err` to the collector. The drop guard
//! cancels the same token when this task exits, which drops any in-flight host
//! handler future.

use std::sync::Arc;

use tokio::io::BufReader;
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::{Mutex, mpsc};

use crate::protocol::{KernelEvent, KernelRequest};

use super::{HostRequestHandler, deny_host_handler};

/// Number of decoded events the pump may buffer ahead of the collector.
///
/// The channel is bounded on purpose. If the collector is slow, the pump stops
/// reading stdout, the kernel's next protocol write blocks on the pipe, and
/// host memory stays bounded. The capacity is large enough to absorb a burst
/// of `print()` frames without adding a round trip per line.
pub(super) const EVENT_CHANNEL_CAPACITY: usize = 128;

/// The request currently allowed to own the kernel's stdout.
///
/// The session installs this value before writing an `execute` request and the
/// pump clears it when the matching `done` arrives. Keeping the id and the
/// handler together is what makes the host callback a *capability*: a request
/// cannot answer with a handler that belongs to a different turn.
#[derive(Clone)]
pub(super) struct ActiveRequestContext {
    /// Request id (`c1`, `s1`, ...) echoed by every event for this request.
    pub id: String,
    /// Host callback captured when the request started.
    ///
    /// Cloning the `Arc` keeps the handler alive even if the session installs a
    /// new handler for a later turn before this request's `done`. `None` is
    /// meaningful: the request may own stdout but is not allowed to call back
    /// into the host.
    pub host_handler: Option<HostRequestHandler>,
}

/// Start the session's only stdout reader.
///
/// The caller must have completed the `ready` handshake. `stdout` is moved into
/// the task and is never read by anyone else. The task runs until the child
/// closes stdout, a protocol error occurs, or the event receiver is dropped.
///
/// # Arguments
///
/// - `stdout`: buffered child stdout; the sole owner of the protocol channel.
/// - `stdin`: shared child stdin. The pump uses it only to write `host_reply`
///   frames, under the same mutex as `execute` / `interrupt` / `shutdown`
///   writes.
/// - `active_request`: the current request capability. The session updates it
///   before writing a request; the pump clears it on the matching `done`.
/// - `idle_bash_handler`: fallback for Bash completion and control while no
///   cell is active.
/// - `event_tx`: decoded events for the session's collector. Bounded; see
///   [`EVENT_CHANNEL_CAPACITY`].
/// - `lifetime`: cancellation token shared with the session. The drop guard
///   cancels it when this task exits; cancelling it externally drops in-flight
///   host handler futures.
///
/// # Routing
///
/// See the module docs. `host_request` frames are answered here and never
/// reach `event_tx`; `done` clears the active request before it is forwarded.
///
/// # Cancellation
///
/// The task has no public handle. It ends when stdout closes, when the
/// collector is dropped, or when the process is shutting down. In-flight host
/// handlers observe the shared `lifetime` token and must release request-local
/// resources when their future is dropped.
pub(super) fn spawn(
    mut stdout: BufReader<ChildStdout>,
    stdin: Arc<Mutex<ChildStdin>>,
    active_request: Arc<Mutex<Option<ActiveRequestContext>>>,
    idle_bash_handler: Arc<Mutex<Option<HostRequestHandler>>>,
    event_tx: mpsc::Sender<Result<KernelEvent, super::ReplError>>,
    lifetime: tokio_util::sync::CancellationToken,
) {
    tokio::spawn(async move {
        let _lifetime_guard = lifetime.clone().drop_guard();
        loop {
            let event = match super::read_line_event(&mut stdout).await {
                Ok(event) => event,
                Err(error) => {
                    lifetime.cancel();
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
                    } else if matches!(
                        action,
                        Some(
                            "bash.completed"
                                | "bash.consumed"
                                | "bash.poll"
                                | "bash.kill"
                                | "bash.forget"
                        )
                    ) {
                        idle_bash_handler
                            .lock()
                            .await
                            .clone()
                            .unwrap_or_else(deny_host_handler)
                    } else {
                        deny_host_handler()
                    };
                    // Capture authorization before spawning: a later Done must not
                    // attach this request to a different turn. Keep reading stdout
                    // while the host awaits approval so EOF cancels owned work.
                    let stdin = Arc::clone(&stdin);
                    let event_tx = event_tx.clone();
                    let lifetime = lifetime.clone();
                    tokio::spawn(async move {
                        let reply = async {
                            let reply_data = handler(id.clone(), data).await;
                            let reply = KernelRequest::HostReply {
                                id,
                                data: reply_data,
                            };
                            let mut stdin = stdin.lock().await;
                            super::write_request(&mut stdin, &reply).await
                        };
                        // Host handlers must release request-local resources when
                        // their future is dropped during kernel teardown.
                        tokio::select! {
                            biased;
                            _ = lifetime.cancelled() => {},
                            result = reply => {
                                if let Err(error) = result {
                                    lifetime.cancel();
                                    let _ = event_tx.send(Err(error)).await;
                                }
                            }
                        }
                    });
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
                            // Clear the capability before forwarding. After
                            // `done`, a late host request must fall back to
                            // idle/deny even if the collector is slow to
                            // consume this event.
                            active_request.take();
                            true
                        } else {
                            // A stale or foreign `done` must never clear the
                            // newer request that currently owns stdout.
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

/// Whether an ordinary event belongs to `request_id`.
///
/// This is the forwarding filter for the `other` arm of the pump loop.
/// `stdout`, `stderr`, `display`, and `error` with `id: None` are
/// session-global and accepted while any request is active; events with a
/// concrete id must match exactly. `result` and `done` always carry an id and
/// must match exactly. `ready` and `host_request` never match: `ready` belongs
/// to the handshake and `host_request` is handled before this filter runs.
///
/// `done` is normally handled by its own arm because it also clears the active
/// request; this function documents the same id rule for completeness.
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

    /// Verifies: a Bash completion that arrives after its cell's `done` reaches
    /// the idle handler, stale cell output is dropped, and a foreign `done`
    /// does not clear the next cell's capability.
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
            for action in ["bash.poll", "bash.kill", "bash.forget"]:
                print(json.dumps({"event":"host_request","id":action,"cell_id":cell_id,"data":{"type":action,"handle_id":"opaque-handle"}}), flush=True)
                control_reply = json.loads(sys.stdin.readline())
                assert control_reply["data"]["result"]["accepted"] is True, control_reply
            print(json.dumps({"event":"host_request","id":"h-start","cell_id":cell_id,"data":{"type":"bash.start","command":"echo denied"}}), flush=True)
            start_reply = json.loads(sys.stdin.readline())
            assert start_reply["data"]["status"] == "error", start_reply
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
                    Some(
                        "bash.completed"
                            | "bash.consumed"
                            | "bash.poll"
                            | "bash.kill"
                            | "bash.forget"
                    )
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
            while idle_calls.load(Ordering::SeqCst) < 5 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("late consumed notice must reach the restricted idle handler");
        assert_eq!(active_calls.load(Ordering::SeqCst), 1);
        assert_eq!(idle_calls.load(Ordering::SeqCst), 5);

        let second = session.execute("second").await.expect("second cell");
        assert_eq!(second.stdout, "next cell intact\n");
        assert_eq!(second.status, "ok");
    }
    /// Verifies: a live host request uses the cell-scoped handler, while a
    /// detached Bash completion after `done` falls back to the idle handler and
    /// leaves the next cell readable.
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
    /// Verifies: a real background Bash completion wakes the idle handler after
    /// the launching cell has finished, and the stdout reader still serves the
    /// next cell.
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
        session
            .set_host_handler(Some(Arc::new(|_id, data| {
                Box::pin(async move {
                    assert_eq!(data["type"], "bash.start");
                    json!({"status":"ok","result":{
                        "handle_id":"test-host-handle", "running":true,
                        "exit_code":null, "output":"", "duration":0.0
                    }})
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
                    assert_eq!(data["handle_id"], "test-host-handle");
                    match data["type"].as_str() {
                        Some("bash.poll") => json!({"status":"ok","result":{
                            "handle_id":"test-host-handle", "running":false,
                            "exit_code":0, "output":"host output", "duration":0.1
                        }}),
                        Some("bash.completed") => {
                            idle_calls.fetch_add(1, Ordering::SeqCst);
                            idle_notify.notify_one();
                            json!({"status":"ok","result":{"accepted":true}})
                        }
                        Some("bash.forget" | "bash.consumed") => {
                            json!({"status":"ok","result":{"accepted":true}})
                        }
                        other => panic!("unexpected idle Bash action: {other:?}"),
                    }
                })
            })))
            .await;

        let output = session
            .execute(
                "from rlm.bash import bash\nhandle = bash('echo host output')\nprint('scheduled')",
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

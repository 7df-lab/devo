//! REPL protocol v3 wire types (kernel `repl.md` protocol).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// Wire protocol version announced in the `ready` event.
pub const PROTOCOL_VERSION: u32 = 3;

/// One request from the host to the kernel, serialized as a single JSON line
/// written to the kernel's stdin.
///
/// Requests run strictly in order, one at a time, except `interrupt` and
/// `host_reply`, which are handled on the reader thread. `execute`, `snapshot`,
/// `restore`, `list_names`, `namespace_brief`, and `shutdown` (when it carries an `id`) each
/// receive exactly one matching [`KernelEvent::Done`] with the same `id`.
/// `interrupt` and `host_reply` are fire-and-forget and produce no reply.
///
/// # Serialized examples
///
/// ```json
/// {"type":"execute","id":"c1","code":"x = 1"}
/// {"type":"interrupt","id":"c1"}
/// {"type":"interrupt"}
/// {"type":"host_reply","id":"h1","data":{"status":"ok","result":{}}}
/// {"type":"snapshot","id":"s1","path":"/tmp/kernel.dill","manifest_path":"/tmp/kernel.json","max_bytes":1000000}
/// {"type":"restore","id":"r1","path":"/tmp/kernel.dill"}
/// {"type":"list_names","id":"n1"}
/// {"type":"shutdown","id":"q1"}
/// {"type":"shutdown"}
/// ```
///
/// Optional fields (`id` on `interrupt` and `shutdown`, and the three
/// `snapshot` limits) are omitted from the wire when `None`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum KernelRequest {
    /// Run `code` as one cell in the persistent namespace.
    ///
    /// `id` scopes every event produced by the cell and is echoed by its final
    /// `done`.
    Execute { id: String, code: String },

    /// Raise `KeyboardInterrupt` in a running or queued cell.
    ///
    /// With `id`, that request only is interrupted. Without it, the running
    /// request is interrupted, or the next queued request if none has started
    /// yet. Produces no reply.
    Interrupt {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },

    /// Answer a kernel [`KernelEvent::HostRequest`] carrying the same `id`.
    ///
    /// `data` is delivered verbatim to the awaiting `rlm.repl.host_request`
    /// call. Produces no reply.
    HostReply { id: String, data: Value },

    /// Serialize the user namespace with dill and write a JSON manifest.
    ///
    /// `path` receives the payload. `manifest_path` receives metadata about the
    /// snapshot. `max_bytes` caps the total payload, `max_variable_bytes` caps
    /// one pickled binding, and `prune_oversized` also deletes bindings that
    /// exceed the per-variable cap. The final `done` reports `saved`,
    /// `skipped`, `pruned`, and `bytes`.
    Snapshot {
        id: String,
        path: String,
        manifest_path: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        max_bytes: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max_variable_bytes: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        prune_oversized: Option<bool>,
    },

    /// Restore names from a snapshot previously written by `Snapshot`.
    ///
    /// A missing file is reported as an ok, empty restore with a `reason`; a
    /// corrupt file is reported as an error. Per-name failures are listed in
    /// `failed`.
    Restore { id: String, path: String },

    /// List the sorted user-defined top-level names in the namespace.
    ///
    /// The matching `done` carries them in `names`.
    ListNames { id: String },

    /// Read a bounded name-only sample without touching namespace values.
    /// The matching `done` carries at most 32 ASCII identifiers in `names`.
    NamespaceBrief { id: String },

    /// Stop the runtime, kill its background Bash process groups, and exit 0.
    ///
    /// With `id`, the kernel emits a final `done` before exiting. Closing stdin
    /// has the same effect.
    Shutdown {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
}

/// One event from the kernel to the host, deserialized from a single JSON line
/// read from the kernel's stdout.
///
/// Events are notifications, not replies: one request can produce many events.
/// Every request with an `id` ends with exactly one [`KernelEvent::Done`]
/// carrying that same `id`, after all of its other events. `ready` is sent
/// once at startup. Events whose `id` is `None` (raw fd writes, user threads,
/// untagged displays) belong to no request.
///
/// # Serialized examples
///
/// ```json
/// {"event":"ready","protocol":3,"python":"3.13.11"}
/// {"event":"stdout","id":"c1","text":"hello\n"}
/// {"event":"stderr","id":null,"text":"warning\n"}
/// {"event":"result","id":"c1","text":"1"}
/// {"event":"display","id":"c1","data":{"text/plain":"..."}}
/// {"event":"host_request","id":"h1","cell_id":"c1","data":{"type":"bash","params":{}}}
/// {"event":"error","id":"c1","ename":"ValueError","evalue":"bad","traceback":["..."]}
/// {"event":"done","id":"c1","status":"ok"}
/// {"event":"done","id":"s1","status":"ok","saved":["x"],"skipped":[],"pruned":[],"bytes":1234}
/// {"event":"done","id":"r1","status":"ok","restored":["x"],"failed":[]}
/// {"event":"done","id":"n1","status":"ok","names":["x"]}
/// ```
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum KernelEvent {
    /// Handshake, sent once before the kernel accepts requests.
    ///
    /// `protocol` must equal [`PROTOCOL_VERSION`]; a mismatch fails the spawn.
    /// `python` is the interpreter version string.
    Ready {
        protocol: u32,
        python: String,
        /// Additive capability: absent on older protocol-v3 runtimes.
        #[serde(default)]
        supports_namespace_brief: bool,
    },

    /// Captured standard output.
    ///
    /// `id` names the cell whose Python execution context performed the write.
    /// It is `None` for raw fd writes, user threads, subprocesses, or output
    /// that cannot be attributed to a cell. Text is incremental; a frame is not
    /// a complete line.
    Stdout { id: Option<String>, text: String },

    /// Captured standard error; same attribution rules as [`KernelEvent::Stdout`].
    Stderr { id: Option<String>, text: String },

    /// `repr` of the cell's trailing expression, when it is not `None`.
    ///
    /// The same value is also bound to `_` in the namespace.
    Result { id: String, text: String },

    /// One display payload from `rlm.repl.emit`: a map of MIME type to JSON.
    ///
    /// `id` rides the spawning cell's task context, so a background task keeps
    /// its creator's id after the cell finishes. User threads emit `None`.
    Display { id: Option<String>, data: Value },

    /// A request from kernel code to the host (bash, web search, MCP, ...).
    ///
    /// `id` is the request id the host must echo in
    /// [`KernelRequest::HostReply`]. `cell_id` is present only while a live
    /// cell owns the request; a late detached request cannot reuse a later
    /// turn's capability. The host denies idle requests except the narrow
    /// `bash.completed` and `bash.consumed` routes.
    HostRequest {
        id: String,
        #[serde(default)]
        cell_id: Option<String>,
        data: Value,
    },

    /// An exception raised by a cell or by the protocol itself.
    ///
    /// `id` is `None` for protocol-level errors. An error does not end the
    /// request by itself; the matching `done` follows.
    Error {
        id: Option<String>,
        ename: String,
        evalue: String,
        #[serde(default)]
        traceback: Vec<String>,
    },

    /// Final event for one id'd request; always the last event with that `id`.
    ///
    /// `status` is `"ok"` or `"error"`. `names` carries the result of
    /// `list_names`. `reason` explains a failed snapshot/restore or a missing
    /// snapshot. `extra` preserves operation-specific fields (`saved`,
    /// `skipped`, `pruned`, `bytes`, `restored`, `failed`) that are flattened
    /// into the top level.
    Done {
        id: String,
        status: String,
        #[serde(default)]
        names: Option<Vec<String>>,
        #[serde(default)]
        reason: Option<String>,
        #[serde(flatten)]
        extra: Value,
    },
}

/// Parsed form of [`KernelEvent::Ready`], returned by the spawn handshake.
///
/// [`KernelSession::spawn`](crate::KernelSession::spawn) waits for this event
/// before returning, so every live session has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadyEvent {
    /// Protocol version the kernel speaks. Must equal [`PROTOCOL_VERSION`].
    pub protocol: u32,
    /// Python interpreter version, for example `"3.13.11"`.
    pub python: String,
    /// Explicit support for the bounded name-only request; false on older v3.
    pub supports_namespace_brief: bool,
}

/// Errors from spawning a kernel or talking to it over the protocol.
///
/// I/O and JSON failures are converted transparently from the underlying
/// errors; the other variants describe protocol-level failure modes.
#[derive(Debug, Error)]
pub enum ReplError {
    /// The first event was not `ready`, or the kernel closed stdout during the
    /// handshake. The string keeps the unexpected event or the startup stderr
    /// tail used to diagnose it.
    #[error("protocol handshake failed: {0}")]
    Handshake(String),

    /// The kernel announced a protocol version this host does not speak.
    ///
    /// The host expects [`PROTOCOL_VERSION`].
    #[error("protocol version mismatch: got {got}, expected {PROTOCOL_VERSION}")]
    ProtocolMismatch {
        /// Version announced by the kernel's `ready` event.
        got: u32,
    },

    /// Reading from or writing to the kernel's stdin/stdout failed.
    #[error("kernel I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// A request could not be serialized, or an event could not be parsed.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// The kernel closed its stdout while a request was still in flight.
    #[error("kernel closed unexpectedly")]
    Closed,

    /// The kernel reported a failed cell with this exception.
    #[error("execute failed: {ename}: {evalue}")]
    CellFailed {
        /// Python exception type, for example `"ValueError"`.
        ename: String,
        /// Exception message.
        evalue: String,
    },

    /// Any other protocol-level failure, such as an empty line or a missing
    /// pipe.
    #[error("{0}")]
    Other(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    /// Verifies: execute request serializes to REPL protocol v3 shape.
    #[test]
    fn execute_request_wire_shape() {
        let req = KernelRequest::Execute {
            id: "c1".into(),
            code: "x = 1".into(),
        };
        let line = serde_json::to_string(&req).expect("serialize");
        let v: Value = serde_json::from_str(&line).expect("parse");
        assert_eq!(v["type"], "execute");
        assert_eq!(v["id"], "c1");
        assert_eq!(v["code"], "x = 1");
    }

    /// Verifies: ready event deserializes and exposes protocol version.
    #[test]
    fn ready_event_parses() {
        let raw = r#"{"event":"ready","protocol":3,"python":"3.13.11"}"#;
        let ev: KernelEvent = serde_json::from_str(raw).expect("parse");
        match ev {
            KernelEvent::Ready {
                protocol,
                python,
                supports_namespace_brief,
            } => {
                assert_eq!(
                    (protocol, python.as_str(), supports_namespace_brief),
                    (PROTOCOL_VERSION, "3.13.11", false)
                );
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn host_request_cell_scope_is_optional_for_older_repl_frames() {
        let current: KernelEvent = serde_json::from_value(serde_json::json!({
            "event": "host_request",
            "id": "h1",
            "cell_id": "c1",
            "data": { "type": "bash.completed" },
        }))
        .expect("parse current host request");
        assert!(
            matches!(current, KernelEvent::HostRequest { cell_id: Some(id), .. } if id == "c1")
        );

        let older: KernelEvent = serde_json::from_value(serde_json::json!({
            "event": "host_request",
            "id": "h2",
            "data": { "type": "bash.completed" },
        }))
        .expect("parse older host request");
        assert!(matches!(
            older,
            KernelEvent::HostRequest { cell_id: None, .. }
        ));
    }
}

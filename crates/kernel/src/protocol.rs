//! REPL protocol v3 wire types (kernel `repl.md` protocol).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// Wire protocol version announced in the `ready` event.
pub const PROTOCOL_VERSION: u32 = 3;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum KernelRequest {
    Execute {
        id: String,
        code: String,
    },
    Interrupt {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    HostReply {
        id: String,
        data: Value,
    },
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
    Restore {
        id: String,
        path: String,
    },
    ListNames {
        id: String,
    },
    Shutdown {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum KernelEvent {
    Ready {
        protocol: u32,
        python: String,
    },
    Stdout {
        id: Option<String>,
        text: String,
    },
    Stderr {
        id: Option<String>,
        text: String,
    },
    Result {
        id: String,
        text: String,
    },
    Display {
        id: Option<String>,
        data: Value,
    },
    HostRequest {
        id: String,
        #[serde(default)]
        cell_id: Option<String>,
        data: Value,
    },
    Error {
        id: Option<String>,
        ename: String,
        evalue: String,
        #[serde(default)]
        traceback: Vec<String>,
    },
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadyEvent {
    pub protocol: u32,
    pub python: String,
}

#[derive(Debug, Error)]
pub enum ReplError {
    #[error("protocol handshake failed: {0}")]
    Handshake(String),
    #[error("protocol version mismatch: got {got}, expected {PROTOCOL_VERSION}")]
    ProtocolMismatch { got: u32 },
    #[error("kernel I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("kernel closed unexpectedly")]
    Closed,
    #[error("execute failed: {ename}: {evalue}")]
    CellFailed { ename: String, evalue: String },
    #[error("{0}")]
    Other(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    /// Trace: L2-DES-RLM-001
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

    /// Trace: L2-DES-RLM-001
    /// Verifies: ready event deserializes and exposes protocol version.
    #[test]
    fn ready_event_parses() {
        let raw = r#"{"event":"ready","protocol":3,"python":"3.13.11"}"#;
        let ev: KernelEvent = serde_json::from_str(raw).expect("parse");
        match ev {
            KernelEvent::Ready { protocol, python } => {
                assert_eq!(protocol, PROTOCOL_VERSION);
                assert_eq!(python, "3.13.11");
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

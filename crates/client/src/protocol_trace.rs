//! Wire-level protocol trace for stdio transport.
//!
//! When enabled via `DEVO_PROTOCOL_TRACE=1`, every NDJSON line sent to or
//! received from the server child process is recorded to a structured NDJSONL
//! file. Each record carries a monotonic sequence number, UTC timestamp,
//! direction, byte count, and the raw JSON payload as it appeared on the wire.

use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use chrono::Utc;

/// Direction of the protocol message relative to the client.
#[derive(Clone, Copy)]
pub(crate) enum TraceDirection {
    /// Client → server (outbound to child stdin).
    Out,
    /// Server → client (inbound from child stdout).
    In,
}

impl TraceDirection {
    fn as_str(self) -> &'static str {
        match self {
            TraceDirection::Out => "out",
            TraceDirection::In => "in",
        }
    }
}

struct ProtocolTraceInner {
    file: Mutex<File>,
    seq: AtomicU64,
}

/// Captures raw NDJSON wire traffic to a structured trace file.
///
/// Cheap to clone (wraps an `Arc`). Thread-safe: the writer/reader async tasks
/// each hold a clone and call `record` independently; the shared `AtomicU64`
/// sequence counter ensures a total order across directions.
#[derive(Clone)]
pub(crate) struct ProtocolTrace {
    inner: Arc<ProtocolTraceInner>,
}

impl ProtocolTrace {
    /// Reads `DEVO_PROTOCOL_TRACE` and (optionally) `DEVO_PROTOCOL_TRACE_FILE`
    /// from the environment. Returns `None` when tracing is disabled or when the
    /// trace file cannot be created.
    ///
    /// Called once during [`StdioServerClient::spawn`]; the result is cloned
    /// into the writer and reader tasks.
    pub(crate) fn from_env() -> Option<Self> {
        let enabled = std::env::var("DEVO_PROTOCOL_TRACE")
            .ok()
            .filter(|v| !v.is_empty())
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        if !enabled {
            return None;
        }

        let path = resolve_trace_path();
        match File::create(&path) {
            Ok(file) => {
                tracing::info!(path = %path.display(), "protocol trace enabled");
                Some(Self {
                    inner: Arc::new(ProtocolTraceInner {
                        file: Mutex::new(file),
                        seq: AtomicU64::new(1),
                    }),
                })
            }
            Err(err) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "failed to create protocol trace file; tracing disabled"
                );
                None
            }
        }
    }

    /// Creates a `ProtocolTrace` that writes to the given file. Used in tests.
    #[cfg(test)]
    pub(crate) fn with_file(file: File) -> Self {
        Self {
            inner: Arc::new(ProtocolTraceInner {
                file: Mutex::new(file),
                seq: AtomicU64::new(1),
            }),
        }
    }

    /// Record a single protocol line.
    pub(crate) fn record(&self, dir: TraceDirection, line: &str) {
        let seq = self.inner.seq.fetch_add(1, Ordering::Relaxed);
        let ts = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let bytes = line.len();

        let record = serde_json::json!({
            "seq": seq,
            "ts": ts,
            "dir": dir.as_str(),
            "bytes": bytes,
            "line": line,
        });

        if let Ok(mut f) = self.inner.file.lock() {
            let buf = serde_json::to_vec(&record).expect("serialize trace record");
            let _ = f.write_all(&buf);
            let _ = f.write_all(b"\n");
            let _ = f.flush();
        }
    }
}

fn resolve_trace_path() -> PathBuf {
    if let Ok(explicit) = std::env::var("DEVO_PROTOCOL_TRACE_FILE") {
        if !explicit.is_empty() {
            let path = PathBuf::from(&explicit);
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            return path;
        }
    }

    let base = devo_util_paths::find_devo_home()
        .map(|home| home.join("traces"))
        .unwrap_or_else(|_| {
            let mut tmp = std::env::temp_dir();
            tmp.push("devo-traces");
            tmp
        });
    let _ = fs::create_dir_all(&base);

    let pid = std::process::id();
    let ts = Utc::now().format("%Y%m%dT%H%M%SZ");
    base.join(format!("protocol-{pid}-{ts}.ndjsonl"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::io::{BufRead, BufReader};

    #[test]
    fn from_env_returns_none_when_not_set() {
        // DEVO_PROTOCOL_TRACE is not set in the test environment by default.
        // We cannot safely call remove_var, but the CI/test environment does
        // not set this variable, so from_env should return None.
        if std::env::var("DEVO_PROTOCOL_TRACE").is_err() {
            assert!(ProtocolTrace::from_env().is_none());
        }
    }

    #[test]
    fn records_outbound_and_inbound_with_monotonic_seq() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("trace.ndjsonl");
        let file = File::create(&path).expect("create trace file");
        let trace = ProtocolTrace::with_file(file);

        trace.record(
            TraceDirection::Out,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
        );
        trace.record(
            TraceDirection::In,
            r#"{"jsonrpc":"2.0","id":1,"result":{}}"#,
        );
        trace.record(
            TraceDirection::Out,
            r#"{"jsonrpc":"2.0","id":2,"method":"session/new"}"#,
        );

        let reader = BufReader::new(File::open(&path).expect("open trace file"));
        let records: Vec<serde_json::Value> = reader
            .lines()
            .map(|line| serde_json::from_str(&line.expect("read line")).expect("parse record"))
            .collect();

        assert_eq!(records.len(), 3);

        assert_eq!(records[0]["seq"], 1);
        assert_eq!(records[0]["dir"], "out");
        assert_eq!(
            records[0]["line"],
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#
        );
        assert_eq!(
            records[0]["bytes"],
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#.len()
        );

        assert_eq!(records[1]["seq"], 2);
        assert_eq!(records[1]["dir"], "in");
        assert_eq!(
            records[1]["line"],
            r#"{"jsonrpc":"2.0","id":1,"result":{}}"#
        );

        assert_eq!(records[2]["seq"], 3);
        assert_eq!(records[2]["dir"], "out");

        for record in &records {
            assert!(record["ts"].is_string());
        }
    }
}

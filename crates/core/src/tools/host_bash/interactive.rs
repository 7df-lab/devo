//! Session-scoped controls for already-authorized interactive commands.
use super::*;
use base64::Engine;

/// Ordinary commands remain turn/kernel-owned; interactive commands are session-owned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BashMode {
    Pipe,
    Interactive,
    Pty,
}
impl BashMode {
    pub fn parse(input: &Value) -> Result<Self, String> {
        let mode = match input.get("mode") {
            None => Self::Pipe,
            Some(Value::String(mode)) => match mode.as_str() {
                "pipe" => Self::Pipe,
                "interactive" => Self::Interactive,
                "pty" => Self::Pty,
                _ => return Err("bash mode must be pipe, interactive, or pty".into()),
            },
            Some(_) => return Err("bash mode must be a string".into()),
        };
        if input
            .get("tty")
            .is_some_and(|tty| *tty != json!(mode == Self::Pty))
        {
            return Err("select PTY using mode=pty, not a conflicting tty flag".into());
        }
        Ok(mode)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pipe => "pipe",
            Self::Interactive => "interactive",
            Self::Pty => "pty",
        }
    }
}

/// Control admitted I/O only. Ownership is resolved before awaiting; registry locks
/// are never held during a write or long poll. No action can spawn a new process.
pub async fn control_async(session_id: &str, action: &str, params: &Value) -> Value {
    if matches!(
        action,
        "bash.list" | "bash.attach" | "bash.poll" | "bash.kill" | "bash.forget"
    ) {
        return control(session_id, action, params);
    }
    let Some(id) = params.get("handle_id").and_then(Value::as_str) else {
        return error("missing bash handle_id");
    };
    let io = {
        let handles = HANDLES.lock().unwrap();
        let Some(entry) = handles.get(id) else {
            return error("unknown bash handle");
        };
        let state = entry.lock().unwrap();
        if state.session_id != session_id {
            return error("unknown bash handle");
        }
        let Some(io) = &state.io else {
            return error("bash handle is not interactive");
        };
        io.clone()
    };
    let result = match action {
        "bash.write" => {
            let Some(encoded) = params.get("data").and_then(Value::as_str) else {
                return error("bash.write requires base64 data");
            };
            if encoded.len() > 87_384 {
                return error("bash.write exceeds 65536 bytes");
            }
            let bytes = match base64::engine::general_purpose::STANDARD.decode(encoded) {
                Ok(bytes) if bytes.len() <= 65_536 => bytes,
                Ok(_) => return error("bash.write exceeds 65536 bytes"),
                Err(_) => return error("bash.write requires valid base64 data"),
            };
            io.write(bytes)
                .await
                .map(|written| json!({"written":written}))
        }
        "bash.close_stdin" => io.close_stdin().await.map(|()| json!({"closed":true})),
        "bash.resize" => {
            let dimensions = ["cols", "rows"].map(|key| params.get(key).and_then(Value::as_u64));
            let [Some(cols @ 1..=1000), Some(rows @ 1..=1000)] = dimensions else {
                return error("bash.resize requires cols and rows between 1 and 1000");
            };
            io.resize(cols as u16, rows as u16)
                .await
                .map(|()| json!({"resized":true}))
        }
        "bash.read" => {
            let get = |key, default| match params.get(key) {
                None => Some(default),
                Some(value) => value.as_u64(),
            };
            let (Some(cursor), Some(max_bytes @ 1..=65_536), Some(wait_ms @ 0..=30_000)) =
                (get("cursor", 0), get("max_bytes", 8192), get("wait_ms", 0))
            else {
                return error("invalid read cursor, max_bytes (1..65536), or wait_ms (0..30000)");
            };
            let result = io.read(cursor, max_bytes as usize, wait_ms).await;
            if let Some(message) = result.get("error").and_then(Value::as_str) {
                return error(message);
            }
            Ok(result)
        }
        _ => Err("unknown interactive bash action".into()),
    };
    match result {
        Ok(result) => json!({"status":"ok", "result":result}),
        Err(message) => error(&message),
    }
}

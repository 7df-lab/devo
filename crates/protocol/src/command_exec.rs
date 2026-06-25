use std::path::PathBuf;

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use crate::SessionId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CommandExecTerminalSize {
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandExecProgram {
    OneShot { command: String },
    InteractiveShell,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CommandExecParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    pub process_id: String,
    pub cwd: Option<PathBuf>,
    pub program: CommandExecProgram,
    pub size: Option<CommandExecTerminalSize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CommandExecResult {
    pub process_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CommandExecWriteParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    pub process_id: String,
    pub delta_base64: Option<String>,
    #[serde(default)]
    pub close_stdin: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CommandExecWriteResult {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CommandExecResizeParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    pub process_id: String,
    pub size: CommandExecTerminalSize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CommandExecResizeResult {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CommandExecTerminateParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    pub process_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CommandExecTerminateResult {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum CommandExecOutputStream {
    Pty,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CommandExecOutputDeltaPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    pub process_id: String,
    pub stream: CommandExecOutputStream,
    pub delta_base64: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CommandExecExitedPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    pub process_id: String,
    pub exit_code: Option<i32>,
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn command_exec_params_roundtrip_interactive_shell() {
        let params = CommandExecParams {
            session_id: Some(SessionId::new()),
            process_id: "shell-1".to_string(),
            cwd: Some("/tmp".into()),
            program: CommandExecProgram::InteractiveShell,
            size: Some(CommandExecTerminalSize {
                rows: 24,
                cols: 120,
            }),
        };

        let json = serde_json::to_value(&params).expect("serialize params");
        let restored: CommandExecParams = serde_json::from_value(json).expect("deserialize params");

        assert_eq!(restored, params);
    }

    #[test]
    fn command_exec_output_delta_roundtrips_base64_payload() {
        let payload = CommandExecOutputDeltaPayload {
            session_id: Some(SessionId::new()),
            process_id: "shell-1".to_string(),
            stream: CommandExecOutputStream::Pty,
            delta_base64: "SGVsbG8K".to_string(),
        };

        let json = serde_json::to_value(&payload).expect("serialize payload");
        let restored: CommandExecOutputDeltaPayload =
            serde_json::from_value(json).expect("deserialize payload");

        assert_eq!(restored, payload);
    }

    /// Trace: L2-DES-APP-003
    /// Verifies: missing session_id on command/exec params deserializes to None.
    #[test]
    fn command_exec_params_missing_session_defaults_to_none() {
        let json = serde_json::json!({
            "process_id": "shell-1",
            "cwd": "/tmp",
            "program": {
                "type": "one_shot",
                "command": "pwd"
            },
            "size": null
        });

        let restored: CommandExecParams = serde_json::from_value(json).expect("deserialize params");

        assert_eq!(
            restored,
            CommandExecParams {
                session_id: None,
                process_id: "shell-1".to_string(),
                cwd: Some("/tmp".into()),
                program: CommandExecProgram::OneShot {
                    command: "pwd".to_string(),
                },
                size: None,
            }
        );
    }

    /// Trace: L2-DES-APP-003
    /// Verifies: sessionless command/exec output and exit notifications round-trip.
    #[test]
    fn command_exec_sessionless_notifications_roundtrip() {
        let output = CommandExecOutputDeltaPayload {
            session_id: None,
            process_id: "shell-1".to_string(),
            stream: CommandExecOutputStream::Pty,
            delta_base64: "SGVsbG8K".to_string(),
        };
        let exited = CommandExecExitedPayload {
            session_id: None,
            process_id: "shell-1".to_string(),
            exit_code: Some(0),
        };

        let restored_output: CommandExecOutputDeltaPayload =
            serde_json::from_value(serde_json::to_value(&output).expect("serialize output"))
                .expect("deserialize output");
        let restored_exited: CommandExecExitedPayload =
            serde_json::from_value(serde_json::to_value(&exited).expect("serialize exited"))
                .expect("deserialize exited");

        assert_eq!((restored_output, restored_exited), (output, exited));
    }
}

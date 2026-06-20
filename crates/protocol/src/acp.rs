use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

use crate::AcpAuthCapabilities;
use crate::AcpAuthMethod;
use crate::CommandExecutionPayload;
use crate::FileChangePayload;
use crate::InputItem;
use crate::ItemDeltaKind;
use crate::ItemEventPayload;
use crate::ItemKind;
use crate::ServerEvent;
use crate::SessionEventPayload;
use crate::SessionId;
use crate::ToolCallPayload;
use crate::ToolResultPayload;
use crate::TurnPlanStepPayload;

pub const ACP_INITIALIZE_METHOD: &str = "initialize";
pub const ACP_AUTHENTICATE_METHOD: &str = "authenticate";
pub const ACP_LOGOUT_METHOD: &str = "logout";
pub const ACP_SESSION_NEW_METHOD: &str = "session/new";
pub const ACP_SESSION_LIST_METHOD: &str = "session/list";
pub const ACP_SESSION_LOAD_METHOD: &str = "session/load";
pub const ACP_SESSION_RESUME_METHOD: &str = "session/resume";
pub const ACP_SESSION_CLOSE_METHOD: &str = "session/close";
pub const ACP_SESSION_DELETE_METHOD: &str = "session/delete";
pub const ACP_SESSION_PROMPT_METHOD: &str = "session/prompt";
pub const ACP_SESSION_CANCEL_METHOD: &str = "session/cancel";
pub const ACP_SESSION_UPDATE_METHOD: &str = "session/update";
pub const ACP_SESSION_REQUEST_PERMISSION_METHOD: &str = "session/request_permission";
pub const ACP_SESSION_SET_MODE_METHOD: &str = "session/set_mode";
pub const ACP_SESSION_SET_CONFIG_OPTION_METHOD: &str = "session/set_config_option";
pub const ACP_FS_READ_TEXT_FILE_METHOD: &str = "fs/read_text_file";
pub const ACP_FS_WRITE_TEXT_FILE_METHOD: &str = "fs/write_text_file";
pub const ACP_TERMINAL_CREATE_METHOD: &str = "terminal/create";
pub const ACP_TERMINAL_OUTPUT_METHOD: &str = "terminal/output";
pub const ACP_TERMINAL_WAIT_FOR_EXIT_METHOD: &str = "terminal/wait_for_exit";
pub const ACP_TERMINAL_KILL_METHOD: &str = "terminal/kill";
pub const ACP_TERMINAL_RELEASE_METHOD: &str = "terminal/release";
pub const ACP_JSONRPC_VERSION: &str = "2.0";
pub const DEVO_EXTENSION_METHOD_PREFIX: &str = "_devo/";
pub const DEVO_ORIGINAL_METHOD_META: &str = "devo/originalMethod";
pub const DEVO_ORIGINAL_EVENT_META: &str = "devo/originalEvent";
pub const DEVO_SESSION_META: &str = "devo/session";
pub const DEVO_SESSION_RESUME_META: &str = "devo/sessionResume";

pub type AcpMeta = serde_json::Map<String, serde_json::Value>;

fn jsonrpc_version() -> String {
    ACP_JSONRPC_VERSION.to_string()
}

pub fn devo_extension_method(method: &str) -> String {
    format!("{DEVO_EXTENSION_METHOD_PREFIX}{method}")
}

pub fn devo_extension_inner_method(method: &str) -> Option<&str> {
    method.strip_prefix(DEVO_EXTENSION_METHOD_PREFIX)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpClientRequest<T> {
    #[serde(default = "jsonrpc_version")]
    pub jsonrpc: String,
    pub id: serde_json::Value,
    pub method: String,
    pub params: T,
}

impl<T> AcpClientRequest<T> {
    pub fn new(id: serde_json::Value, method: impl Into<String>, params: T) -> Self {
        Self {
            jsonrpc: jsonrpc_version(),
            id,
            method: method.into(),
            params,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpClientNotification<T> {
    #[serde(default = "jsonrpc_version")]
    pub jsonrpc: String,
    pub method: String,
    pub params: T,
}

impl<T> AcpClientNotification<T> {
    pub fn new(method: impl Into<String>, params: T) -> Self {
        Self {
            jsonrpc: jsonrpc_version(),
            method: method.into(),
            params,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpSuccessResponse<T> {
    #[serde(default = "jsonrpc_version")]
    pub jsonrpc: String,
    pub id: serde_json::Value,
    pub result: T,
}

impl<T> AcpSuccessResponse<T> {
    pub fn new(id: serde_json::Value, result: T) -> Self {
        Self {
            jsonrpc: jsonrpc_version(),
            id,
            result,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpErrorResponse {
    #[serde(default = "jsonrpc_version")]
    pub jsonrpc: String,
    pub id: serde_json::Value,
    pub error: AcpProtocolError,
}

impl AcpErrorResponse {
    pub fn new(
        id: serde_json::Value,
        code: AcpErrorCode,
        message: impl Into<String>,
        data: serde_json::Value,
    ) -> Self {
        Self {
            jsonrpc: jsonrpc_version(),
            id,
            error: AcpProtocolError {
                code: code as i64,
                message: message.into(),
                data,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpProtocolError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpErrorCode {
    ParseError = -32700,
    InvalidRequest = -32600,
    MethodNotFound = -32601,
    InvalidParams = -32602,
    InternalError = -32603,
    ServerError = -32000,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpImplementation {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub version: String,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

impl AcpImplementation {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            title: None,
            version: version.into(),
            meta: None,
        }
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpInitializeParams {
    pub protocol_version: u16,
    #[serde(default)]
    pub client_capabilities: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_info: Option<AcpImplementation>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpInitializeResult {
    pub protocol_version: u16,
    #[serde(default)]
    pub agent_capabilities: AcpAgentCapabilities,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auth_methods: Vec<AcpAuthMethod>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_info: Option<AcpImplementation>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpAgentCapabilities {
    #[serde(default)]
    pub load_session: bool,
    #[serde(default)]
    pub prompt_capabilities: AcpPromptCapabilities,
    #[serde(default)]
    pub mcp_capabilities: AcpMcpCapabilities,
    #[serde(default)]
    pub session_capabilities: AcpSessionCapabilities,
    #[serde(default, skip_serializing_if = "AcpAuthCapabilities::is_empty")]
    pub auth: AcpAuthCapabilities,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpPromptCapabilities {
    #[serde(default)]
    pub image: bool,
    #[serde(default)]
    pub audio: bool,
    #[serde(default)]
    pub embedded_context: bool,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpMcpCapabilities {
    #[serde(default)]
    pub http: bool,
    #[serde(default)]
    pub sse: bool,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AcpMcpServer {
    Stdio(AcpMcpServerStdio),
    Unsupported(AcpUnsupportedMcpServer),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpMcpServerStdio {
    pub name: String,
    pub command: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<AcpEnvVariable>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpEnvVariable {
    pub name: String,
    pub value: String,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpUnsupportedMcpServer {
    #[serde(rename = "type")]
    pub transport_type: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpSessionCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delete: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_directories: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close: Option<serde_json::Value>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpNewSessionParams {
    pub cwd: PathBuf,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_directories: Vec<PathBuf>,
    #[serde(default)]
    pub mcp_servers: Vec<AcpMcpServer>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpNewSessionResult {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modes: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_options: Option<Vec<serde_json::Value>>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpPromptParams {
    pub session_id: SessionId,
    pub prompt: Vec<AcpContentBlock>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpPromptResult {
    pub stop_reason: AcpStopReason,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpStopReason {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpCancelParams {
    pub session_id: SessionId,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AcpContentBlock {
    Text {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotations: Option<AcpAnnotations>,
        text: String,
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<AcpMeta>,
    },
    Image {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotations: Option<AcpAnnotations>,
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uri: Option<String>,
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<AcpMeta>,
    },
    Audio {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotations: Option<AcpAnnotations>,
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<AcpMeta>,
    },
    ResourceLink {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotations: Option<AcpAnnotations>,
        uri: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[serde(rename = "mimeType")]
        mime_type: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<i64>,
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<AcpMeta>,
    },
    Resource {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotations: Option<AcpAnnotations>,
        resource: AcpEmbeddedResource,
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<AcpMeta>,
    },
}

impl AcpContentBlock {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text {
            annotations: None,
            text: text.into(),
            meta: None,
        }
    }

    pub fn into_input_items(self) -> Result<Vec<InputItem>, String> {
        match self {
            Self::Text { text, .. } => Ok(vec![InputItem::Text { text }]),
            Self::Image { .. } => {
                Err("session/prompt image content is not supported by this agent".to_string())
            }
            Self::Audio { .. } => {
                Err("session/prompt audio content is not supported by this agent".to_string())
            }
            Self::ResourceLink { uri, name, .. } => {
                if let Some(path) = path_from_file_uri(&uri) {
                    Ok(vec![InputItem::Mention {
                        path: path.to_string_lossy().into_owned(),
                        name: Some(name),
                    }])
                } else {
                    Ok(vec![InputItem::Text {
                        text: format!("Resource {name}: {uri}"),
                    }])
                }
            }
            Self::Resource { resource, .. } => Ok(vec![InputItem::Text {
                text: resource.into_prompt_text(),
            }]),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AcpRole {
    Assistant,
    User,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpAnnotations {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience: Option<Vec<AcpRole>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<f64>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AcpEmbeddedResource {
    Text(AcpTextResourceContents),
    Blob(AcpBlobResourceContents),
}

impl AcpEmbeddedResource {
    fn into_prompt_text(self) -> String {
        match self {
            Self::Text(resource) => format!("Resource {}:\n{}", resource.uri, resource.text),
            Self::Blob(resource) => {
                let mime_type = resource.mime_type.unwrap_or_else(|| "unknown".to_string());
                format!(
                    "Resource {} ({mime_type}; base64):\n{}",
                    resource.uri, resource.blob
                )
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct AcpTextResourceContents {
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    pub text: String,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct AcpBlobResourceContents {
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    pub blob: String,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpSessionNotification {
    pub session_id: SessionId,
    pub update: AcpSessionUpdate,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRequestPermissionParams {
    pub session_id: SessionId,
    pub tool_call: AcpToolCallUpdate,
    pub options: Vec<AcpPermissionOption>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpToolCallUpdate {
    #[serde(rename = "toolCallId")]
    pub tool_call_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<AcpToolKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<AcpToolCallStatus>,
    #[serde(default, rename = "rawInput", skip_serializing_if = "Option::is_none")]
    pub raw_input: Option<serde_json::Value>,
    #[serde(default, rename = "rawOutput", skip_serializing_if = "Option::is_none")]
    pub raw_output: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content: Vec<AcpToolCallContent>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locations: Vec<AcpToolCallLocation>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct AcpToolCallLocation {
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpPermissionOption {
    #[serde(rename = "optionId")]
    pub option_id: String,
    pub name: String,
    pub kind: AcpPermissionOptionKind,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpPermissionOptionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRequestPermissionResponse {
    pub outcome: AcpPermissionOutcome,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum AcpPermissionOutcome {
    Selected {
        #[serde(rename = "optionId")]
        option_id: String,
    },
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "sessionUpdate", rename_all = "snake_case")]
pub enum AcpSessionUpdate {
    UserMessageChunk {
        content: AcpContentBlock,
        #[serde(default, rename = "messageId", skip_serializing_if = "Option::is_none")]
        message_id: Option<String>,
    },
    AgentMessageChunk {
        content: AcpContentBlock,
        #[serde(default, rename = "messageId", skip_serializing_if = "Option::is_none")]
        message_id: Option<String>,
    },
    AgentThoughtChunk {
        content: AcpContentBlock,
        #[serde(default, rename = "messageId", skip_serializing_if = "Option::is_none")]
        message_id: Option<String>,
    },
    ToolCall {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        title: String,
        kind: AcpToolKind,
        status: AcpToolCallStatus,
        #[serde(default, rename = "rawInput", skip_serializing_if = "Option::is_none")]
        raw_input: Option<serde_json::Value>,
        #[serde(default, rename = "rawOutput", skip_serializing_if = "Option::is_none")]
        raw_output: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        content: Vec<AcpToolCallContent>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        locations: Vec<AcpToolCallLocation>,
    },
    ToolCallUpdate {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<AcpToolKind>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<AcpToolCallStatus>,
        #[serde(default, rename = "rawInput", skip_serializing_if = "Option::is_none")]
        raw_input: Option<serde_json::Value>,
        #[serde(default, rename = "rawOutput", skip_serializing_if = "Option::is_none")]
        raw_output: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        content: Vec<AcpToolCallContent>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        locations: Vec<AcpToolCallLocation>,
    },
    Plan {
        entries: Vec<AcpPlanEntry>,
    },
    SessionInfoUpdate {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, rename = "updatedAt", skip_serializing_if = "Option::is_none")]
        updated_at: Option<String>,
    },
    UsageUpdate {
        used: u64,
        size: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpToolCallStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AcpToolCallContent {
    Content {
        content: AcpContentBlock,
    },
    Diff {
        path: PathBuf,
        #[serde(default, rename = "oldText", skip_serializing_if = "Option::is_none")]
        old_text: Option<String>,
        #[serde(rename = "newText")]
        new_text: String,
    },
    Terminal {
        #[serde(rename = "terminalId")]
        terminal_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct AcpFsReadTextFileParams {
    pub session_id: SessionId,
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpFsReadTextFileResult {
    pub content: String,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct AcpFsWriteTextFileParams {
    pub session_id: SessionId,
    pub path: PathBuf,
    pub content: String,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct AcpTerminalCreateParams {
    pub session_id: SessionId,
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<AcpEnvVariable>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(
        default,
        rename = "outputByteLimit",
        skip_serializing_if = "Option::is_none"
    )]
    pub output_byte_limit: Option<usize>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpTerminalCreateResult {
    #[serde(rename = "terminalId")]
    pub terminal_id: String,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct AcpTerminalParams {
    pub session_id: SessionId,
    #[serde(rename = "terminalId")]
    pub terminal_id: String,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpTerminalOutputResult {
    pub output: String,
    pub truncated: bool,
    #[serde(
        default,
        rename = "exitStatus",
        skip_serializing_if = "Option::is_none"
    )]
    pub exit_status: Option<AcpTerminalExitStatus>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpTerminalWaitForExitResult {
    #[serde(default, rename = "exitCode", skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<String>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpTerminalExitStatus {
    #[serde(default, rename = "exitCode", skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpPlanEntry {
    pub content: String,
    pub priority: AcpPlanEntryPriority,
    pub status: AcpPlanEntryStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpPlanEntryPriority {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpPlanEntryStatus {
    Pending,
    InProgress,
    Completed,
}

pub fn input_items_from_acp_prompt(prompt: Vec<AcpContentBlock>) -> Result<Vec<InputItem>, String> {
    let mut input = Vec::new();
    for block in prompt {
        input.extend(block.into_input_items()?);
    }
    Ok(input)
}

pub fn acp_notification_from_server_event(
    method: &str,
    event: &ServerEvent,
) -> (String, serde_json::Value) {
    let Some(session_id) = event.session_id() else {
        return (
            devo_extension_method(method),
            serde_json::to_value(event).expect("serialize devo extension event"),
        );
    };
    let update = acp_update_from_server_event(event).unwrap_or({
        AcpSessionUpdate::SessionInfoUpdate {
            title: None,
            updated_at: None,
        }
    });
    let mut meta = AcpMeta::new();
    meta.insert(
        DEVO_ORIGINAL_METHOD_META.to_string(),
        serde_json::Value::String(method.to_string()),
    );
    meta.insert(
        DEVO_ORIGINAL_EVENT_META.to_string(),
        serde_json::to_value(event).expect("serialize original server event"),
    );
    (
        ACP_SESSION_UPDATE_METHOD.to_string(),
        serde_json::to_value(AcpSessionNotification {
            session_id,
            update,
            meta: Some(meta),
        })
        .expect("serialize ACP session update"),
    )
}

pub fn original_event_from_acp_notification(
    notification: &AcpSessionNotification,
) -> Option<(String, ServerEvent)> {
    let meta = notification.meta.as_ref()?;
    let method = meta.get(DEVO_ORIGINAL_METHOD_META)?.as_str()?.to_string();
    let event = serde_json::from_value(meta.get(DEVO_ORIGINAL_EVENT_META)?.clone()).ok()?;
    Some((method, event))
}

fn acp_update_from_server_event(event: &ServerEvent) -> Option<AcpSessionUpdate> {
    match event {
        ServerEvent::SessionStarted(SessionEventPayload { session })
        | ServerEvent::SessionTitleUpdated(SessionEventPayload { session }) => {
            Some(AcpSessionUpdate::SessionInfoUpdate {
                title: session.title.clone(),
                updated_at: Some(session.updated_at.to_rfc3339()),
            })
        }
        ServerEvent::TurnPlanUpdated(payload) => Some(AcpSessionUpdate::Plan {
            entries: payload
                .plan
                .iter()
                .map(acp_plan_entry_from_turn_plan_step)
                .collect(),
        }),
        ServerEvent::TurnUsageUpdated(payload) => {
            let used = (payload.total_input_tokens + payload.total_output_tokens) as u64;
            Some(AcpSessionUpdate::UsageUpdate {
                used,
                size: payload.context_window.unwrap_or_else(|| used.max(1)),
            })
        }
        ServerEvent::ToolCallStatusUpdated(payload) => Some(AcpSessionUpdate::ToolCallUpdate {
            tool_call_id: payload.tool_call_id.clone(),
            title: None,
            kind: None,
            status: acp_tool_call_status_from_str(payload.status.as_str()),
            raw_input: None,
            raw_output: None,
            content: Vec::new(),
            locations: Vec::new(),
        }),
        ServerEvent::ItemDelta {
            delta_kind,
            payload,
        } => acp_update_from_item_delta(delta_kind.clone(), payload),
        ServerEvent::ItemStarted(payload) => acp_update_from_item_started(payload),
        ServerEvent::ItemCompleted(payload) => acp_update_from_item_completed(payload),
        _ => None,
    }
}

fn acp_update_from_item_delta(
    delta_kind: ItemDeltaKind,
    payload: &crate::ItemDeltaPayload,
) -> Option<AcpSessionUpdate> {
    let content = AcpContentBlock::text(payload.delta.clone());
    let message_id = payload.context.item_id.map(|item_id| item_id.to_string());
    match delta_kind {
        ItemDeltaKind::AgentMessageDelta => Some(AcpSessionUpdate::AgentMessageChunk {
            content,
            message_id,
        }),
        ItemDeltaKind::ReasoningSummaryTextDelta | ItemDeltaKind::ReasoningTextDelta => {
            Some(AcpSessionUpdate::AgentThoughtChunk {
                content,
                message_id,
            })
        }
        _ => None,
    }
}

fn acp_update_from_item_started(payload: &ItemEventPayload) -> Option<AcpSessionUpdate> {
    match payload.item.item_kind {
        ItemKind::ToolCall => {
            let tool =
                serde_json::from_value::<ToolCallPayload>(payload.item.payload.clone()).ok()?;
            Some(AcpSessionUpdate::ToolCall {
                tool_call_id: tool.tool_call_id,
                title: tool_title(tool.tool_name.as_str(), &tool.parameters),
                kind: tool_kind_from_name(tool.tool_name.as_str()),
                status: AcpToolCallStatus::Pending,
                locations: tool_locations_from_value(&tool.parameters),
                raw_input: Some(tool.parameters),
                raw_output: None,
                content: Vec::new(),
            })
        }
        ItemKind::CommandExecution => {
            let command =
                serde_json::from_value::<CommandExecutionPayload>(payload.item.payload.clone())
                    .ok()?;
            Some(AcpSessionUpdate::ToolCall {
                tool_call_id: command.tool_call_id,
                title: command.command,
                kind: AcpToolKind::Execute,
                status: AcpToolCallStatus::Pending,
                locations: command
                    .input
                    .as_ref()
                    .map(tool_locations_from_value)
                    .unwrap_or_default(),
                raw_input: command.input,
                raw_output: None,
                content: Vec::new(),
            })
        }
        _ => None,
    }
}

fn acp_update_from_item_completed(payload: &ItemEventPayload) -> Option<AcpSessionUpdate> {
    match payload.item.item_kind {
        ItemKind::ToolResult => {
            let result =
                serde_json::from_value::<ToolResultPayload>(payload.item.payload.clone()).ok()?;
            Some(AcpSessionUpdate::ToolCallUpdate {
                tool_call_id: result.tool_call_id,
                title: Some(
                    (!result.summary.is_empty())
                        .then_some(result.summary)
                        .or(result.tool_name.clone())
                        .unwrap_or_else(|| "Tool result".to_string()),
                ),
                kind: result.tool_name.as_deref().map(tool_kind_from_name),
                status: Some(if result.is_error {
                    AcpToolCallStatus::Failed
                } else {
                    AcpToolCallStatus::Completed
                }),
                raw_input: result.input.clone(),
                raw_output: Some(result.content.clone()),
                locations: result
                    .input
                    .as_ref()
                    .map(tool_locations_from_value)
                    .unwrap_or_default(),
                content: tool_result_content(result.display_content, result.content),
            })
        }
        ItemKind::CommandExecution => {
            let command =
                serde_json::from_value::<CommandExecutionPayload>(payload.item.payload.clone())
                    .ok()?;
            Some(AcpSessionUpdate::ToolCallUpdate {
                tool_call_id: command.tool_call_id,
                title: Some(command.command),
                kind: Some(AcpToolKind::Execute),
                status: Some(if command.is_error {
                    AcpToolCallStatus::Failed
                } else {
                    AcpToolCallStatus::Completed
                }),
                raw_input: command.input,
                raw_output: command.output,
                content: Vec::new(),
                locations: Vec::new(),
            })
        }
        ItemKind::FileChange => {
            let change =
                serde_json::from_value::<FileChangePayload>(payload.item.payload.clone()).ok()?;
            Some(AcpSessionUpdate::ToolCallUpdate {
                tool_call_id: change.tool_call_id.clone(),
                title: change.tool_name.clone(),
                kind: Some(AcpToolKind::Edit),
                status: Some(if change.is_error {
                    AcpToolCallStatus::Failed
                } else {
                    AcpToolCallStatus::Completed
                }),
                raw_input: change.input.clone(),
                raw_output: Some(payload.item.payload.clone()),
                content: file_change_tool_content(&change),
                locations: file_change_locations(&change),
            })
        }
        _ => None,
    }
}

fn acp_plan_entry_from_turn_plan_step(step: &TurnPlanStepPayload) -> AcpPlanEntry {
    AcpPlanEntry {
        content: step.step.clone(),
        priority: AcpPlanEntryPriority::Medium,
        status: match step.status.as_str() {
            "completed" => AcpPlanEntryStatus::Completed,
            "in_progress" => AcpPlanEntryStatus::InProgress,
            "pending" | "cancelled" => AcpPlanEntryStatus::Pending,
            _ => AcpPlanEntryStatus::Pending,
        },
    }
}

fn tool_title(tool_name: &str, parameters: &serde_json::Value) -> String {
    parameters
        .get("command")
        .or_else(|| parameters.get("cmd"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| tool_name.to_string())
}

fn tool_kind_from_name(tool_name: &str) -> AcpToolKind {
    match tool_name {
        "read" | "grep" | "glob" | "lsp" => AcpToolKind::Read,
        "apply_patch" | "edit" | "write" => AcpToolKind::Edit,
        "bash" | "shell_command" | "exec_command" => AcpToolKind::Execute,
        "websearch" | "web_fetch" | "websearch_query" => AcpToolKind::Fetch,
        "agent" => AcpToolKind::Think,
        _ => AcpToolKind::Other,
    }
}

fn acp_tool_call_status_from_str(status: &str) -> Option<AcpToolCallStatus> {
    Some(match status {
        "pending" => AcpToolCallStatus::Pending,
        "in_progress" => AcpToolCallStatus::InProgress,
        "completed" => AcpToolCallStatus::Completed,
        "failed" => AcpToolCallStatus::Failed,
        "cancelled" => AcpToolCallStatus::Cancelled,
        _ => return None,
    })
}

fn file_change_tool_content(change: &FileChangePayload) -> Vec<AcpToolCallContent> {
    change
        .changes
        .iter()
        .map(|(path, change)| match change {
            crate::protocol::FileChange::Add { content } => AcpToolCallContent::Diff {
                path: path.clone(),
                old_text: None,
                new_text: content.clone(),
            },
            crate::protocol::FileChange::Delete { content } => AcpToolCallContent::Diff {
                path: path.clone(),
                old_text: Some(content.clone()),
                new_text: String::new(),
            },
            crate::protocol::FileChange::Update { unified_diff, .. } => {
                AcpToolCallContent::Content {
                    content: AcpContentBlock::text(unified_diff.clone()),
                }
            }
        })
        .collect()
}

fn file_change_locations(change: &FileChangePayload) -> Vec<AcpToolCallLocation> {
    change
        .changes
        .iter()
        .map(|(path, _)| AcpToolCallLocation {
            path: path.clone(),
            line: None,
        })
        .collect()
}

fn tool_locations_from_value(value: &serde_json::Value) -> Vec<AcpToolCallLocation> {
    let mut locations = Vec::new();
    for key in ["path", "filePath", "file_path"] {
        if let Some(path) = value.get(key).and_then(serde_json::Value::as_str) {
            locations.push(AcpToolCallLocation {
                path: PathBuf::from(path),
                line: value.get("line").and_then(serde_json::Value::as_u64),
            });
        }
    }
    for key in ["paths", "files"] {
        if let Some(items) = value.get(key).and_then(serde_json::Value::as_array) {
            for item in items {
                if let Some(path) = item.as_str() {
                    locations.push(AcpToolCallLocation {
                        path: PathBuf::from(path),
                        line: None,
                    });
                } else {
                    push_location_from_object(item, &mut locations);
                }
            }
        }
    }
    locations
}

fn push_location_from_object(value: &serde_json::Value, locations: &mut Vec<AcpToolCallLocation>) {
    let Some(object) = value.as_object() else {
        return;
    };
    let path = object
        .get("path")
        .or_else(|| object.get("filePath"))
        .or_else(|| object.get("file_path"))
        .and_then(serde_json::Value::as_str);
    if let Some(path) = path {
        locations.push(AcpToolCallLocation {
            path: PathBuf::from(path),
            line: object.get("line").and_then(serde_json::Value::as_u64),
        });
    }
}

fn tool_result_content(
    display_content: Option<String>,
    content: serde_json::Value,
) -> Vec<AcpToolCallContent> {
    if let Some(display_content) = display_content {
        return vec![AcpToolCallContent::Content {
            content: AcpContentBlock::text(display_content),
        }];
    }

    if let Some(content) = acp_tool_content_from_value(&content) {
        return content;
    }

    let text = match content {
        serde_json::Value::String(text) => text,
        other => other.to_string(),
    };
    vec![AcpToolCallContent::Content {
        content: AcpContentBlock::text(text),
    }]
}

fn acp_tool_content_from_value(value: &serde_json::Value) -> Option<Vec<AcpToolCallContent>> {
    if let Ok(content) = serde_json::from_value::<AcpToolCallContent>(value.clone()) {
        return Some(vec![content]);
    }

    if let Ok(contents) = serde_json::from_value::<Vec<AcpToolCallContent>>(value.clone()) {
        return Some(contents);
    }

    if let Ok(content) = serde_json::from_value::<AcpContentBlock>(value.clone()) {
        return Some(vec![AcpToolCallContent::Content { content }]);
    }

    if let Ok(contents) = serde_json::from_value::<Vec<AcpContentBlock>>(value.clone()) {
        return Some(
            contents
                .into_iter()
                .map(|content| AcpToolCallContent::Content { content })
                .collect(),
        );
    }

    let mcp_contents = value.get("content")?;
    let contents = serde_json::from_value::<Vec<AcpContentBlock>>(mcp_contents.clone()).ok()?;
    Some(
        contents
            .into_iter()
            .map(|content| AcpToolCallContent::Content { content })
            .collect(),
    )
}

fn path_from_file_uri(uri: &str) -> Option<PathBuf> {
    let path = uri.strip_prefix("file://")?;
    #[cfg(windows)]
    {
        let path = path.strip_prefix('/').unwrap_or(path);
        Some(PathBuf::from(path.replace('/', "\\")))
    }
    #[cfg(not(windows))]
    {
        Some(PathBuf::from(path))
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::EventContext;
    use crate::ItemDeltaPayload;
    use crate::ItemId;
    use crate::SessionId;
    use crate::TurnId;

    fn native_absolute_test_path(suffix: &str) -> String {
        #[cfg(windows)]
        {
            format!(r"C:\Users\user\{}", suffix.replace('/', r"\"))
        }
        #[cfg(unix)]
        {
            format!("/home/user/{suffix}")
        }
    }

    #[test]
    fn initialize_result_uses_acp_field_names() {
        let result = AcpInitializeResult {
            protocol_version: 1,
            agent_capabilities: AcpAgentCapabilities {
                prompt_capabilities: AcpPromptCapabilities {
                    embedded_context: true,
                    ..AcpPromptCapabilities::default()
                },
                ..AcpAgentCapabilities::default()
            },
            auth_methods: Vec::new(),
            agent_info: Some(AcpImplementation::new("devo", "1.2.3").with_title("Devo")),
            meta: None,
        };

        let json = serde_json::to_value(result).expect("serialize initialize result");

        assert_eq!(
            json,
            serde_json::json!({
                "protocolVersion": 1,
                "agentCapabilities": {
                    "loadSession": false,
                    "promptCapabilities": {
                        "image": false,
                        "audio": false,
                        "embeddedContext": true
                    },
                    "mcpCapabilities": {
                        "http": false,
                        "sse": false
                    },
                    "sessionCapabilities": {}
                },
                "agentInfo": {
                    "name": "devo",
                    "title": "Devo",
                    "version": "1.2.3"
                }
            })
        );
    }

    #[test]
    fn fs_text_file_params_use_acp_wire_shape() {
        let session_id = SessionId::new();
        let path = PathBuf::from(native_absolute_test_path("project/src/main.rs"));
        let path_json = serde_json::to_value(&path).expect("serialize path");

        let read_params = AcpFsReadTextFileParams {
            session_id,
            path: path.clone(),
            line: Some(10),
            limit: Some(50),
            meta: None,
        };
        let read_value = serde_json::to_value(&read_params).expect("serialize read params");
        assert_eq!(
            read_value,
            serde_json::json!({
                "sessionId": session_id,
                "path": path_json.clone(),
                "line": 10,
                "limit": 50
            })
        );
        assert_eq!(
            serde_json::from_value::<AcpFsReadTextFileParams>(read_value)
                .expect("deserialize read params"),
            read_params
        );

        let read_result = AcpFsReadTextFileResult {
            content: "def hello_world():\n    print('Hello, world!')\n".to_string(),
            meta: None,
        };
        assert_eq!(
            serde_json::to_value(&read_result).expect("serialize read result"),
            serde_json::json!({
                "content": "def hello_world():\n    print('Hello, world!')\n"
            })
        );

        let write_params = AcpFsWriteTextFileParams {
            session_id,
            path,
            content: "{\n  \"debug\": true\n}".to_string(),
            meta: None,
        };
        let write_value = serde_json::to_value(&write_params).expect("serialize write params");
        assert_eq!(
            write_value,
            serde_json::json!({
                "sessionId": session_id,
                "path": path_json,
                "content": "{\n  \"debug\": true\n}"
            })
        );
        assert_eq!(
            serde_json::from_value::<AcpFsWriteTextFileParams>(write_value)
                .expect("deserialize write params"),
            write_params
        );
    }

    #[test]
    fn new_session_params_accepts_stdio_mcp_server_shape() {
        #[cfg(windows)]
        let cwd = r"C:\Users\user\project";
        #[cfg(windows)]
        let command = r"C:\mcp\filesystem.exe";
        #[cfg(unix)]
        let cwd = "/home/user/project";
        #[cfg(unix)]
        let command = "/path/to/mcp-server";

        let params: AcpNewSessionParams = serde_json::from_value(serde_json::json!({
            "cwd": cwd,
            "mcpServers": [
                {
                    "name": "filesystem",
                    "command": command,
                    "args": ["--stdio"],
                    "env": []
                }
            ]
        }))
        .expect("deserialize ACP session/new params");

        assert_eq!(
            params,
            AcpNewSessionParams {
                cwd: PathBuf::from(cwd),
                additional_directories: Vec::new(),
                mcp_servers: vec![AcpMcpServer::Stdio(AcpMcpServerStdio {
                    name: "filesystem".to_string(),
                    command: PathBuf::from(command),
                    args: vec!["--stdio".to_string()],
                    env: Vec::new(),
                    meta: None,
                })],
                meta: None,
            }
        );
    }

    #[test]
    fn content_blocks_use_acp_content_shapes() {
        let text: AcpContentBlock = serde_json::from_value(serde_json::json!({
            "type": "text",
            "text": "hello",
            "annotations": {
                "audience": ["user"],
                "lastModified": "2026-06-17T00:00:00Z",
                "priority": 0.7
            }
        }))
        .expect("deserialize text content");
        assert_eq!(
            text,
            AcpContentBlock::Text {
                annotations: Some(AcpAnnotations {
                    audience: Some(vec![AcpRole::User]),
                    last_modified: Some("2026-06-17T00:00:00Z".to_string()),
                    priority: Some(0.7),
                    meta: None,
                }),
                text: "hello".to_string(),
                meta: None,
            }
        );

        assert_eq!(
            serde_json::to_value(AcpContentBlock::Image {
                annotations: None,
                data: "iVBORw0KGgo=".to_string(),
                mime_type: "image/png".to_string(),
                uri: Some("file:///tmp/image.png".to_string()),
                meta: None,
            })
            .expect("serialize image content"),
            serde_json::json!({
                "type": "image",
                "data": "iVBORw0KGgo=",
                "mimeType": "image/png",
                "uri": "file:///tmp/image.png"
            })
        );

        assert_eq!(
            serde_json::from_value::<AcpContentBlock>(serde_json::json!({
                "type": "audio",
                "data": "UklGRg==",
                "mimeType": "audio/wav"
            }))
            .expect("deserialize audio content"),
            AcpContentBlock::Audio {
                annotations: None,
                data: "UklGRg==".to_string(),
                mime_type: "audio/wav".to_string(),
                meta: None,
            }
        );

        assert_eq!(
            serde_json::to_value(AcpContentBlock::ResourceLink {
                annotations: None,
                uri: "file:///tmp/document.pdf".to_string(),
                name: "document.pdf".to_string(),
                title: Some("Document".to_string()),
                description: Some("A PDF".to_string()),
                mime_type: Some("application/pdf".to_string()),
                size: Some(1024),
                meta: None,
            })
            .expect("serialize resource link"),
            serde_json::json!({
                "type": "resource_link",
                "uri": "file:///tmp/document.pdf",
                "name": "document.pdf",
                "title": "Document",
                "description": "A PDF",
                "mimeType": "application/pdf",
                "size": 1024
            })
        );
    }

    #[test]
    fn embedded_resource_union_rejects_ambiguous_resource_contents() {
        let invalid = serde_json::json!({
            "type": "resource",
            "resource": {
                "uri": "file:///tmp/data.bin",
                "text": "hello",
                "blob": "AA=="
            }
        });

        assert!(serde_json::from_value::<AcpContentBlock>(invalid).is_err());
    }

    #[test]
    fn acp_prompt_conversion_rejects_unadvertised_image_and_preserves_blob_resource() {
        let error = input_items_from_acp_prompt(vec![AcpContentBlock::Image {
            annotations: None,
            data: "iVBORw0KGgo=".to_string(),
            mime_type: "image/png".to_string(),
            uri: None,
            meta: None,
        }])
        .expect_err("image prompt content should be rejected");
        assert_eq!(
            error,
            "session/prompt image content is not supported by this agent"
        );

        assert_eq!(
            input_items_from_acp_prompt(vec![AcpContentBlock::Resource {
                annotations: None,
                resource: AcpEmbeddedResource::Blob(AcpBlobResourceContents {
                    uri: "file:///tmp/data.bin".to_string(),
                    mime_type: Some("application/octet-stream".to_string()),
                    blob: "AA==".to_string(),
                    meta: None,
                }),
                meta: None,
            }])
            .expect("blob resource converts to prompt text"),
            vec![InputItem::Text {
                text: "Resource file:///tmp/data.bin (application/octet-stream; base64):\nAA=="
                    .to_string()
            }]
        );
    }

    #[test]
    fn tool_result_content_preserves_acp_content_blocks() {
        assert_eq!(
            tool_result_content(
                None,
                serde_json::json!({
                    "content": [
                        {
                            "type": "image",
                            "data": "iVBORw0KGgo=",
                            "mimeType": "image/png"
                        }
                    ]
                })
            ),
            vec![AcpToolCallContent::Content {
                content: AcpContentBlock::Image {
                    annotations: None,
                    data: "iVBORw0KGgo=".to_string(),
                    mime_type: "image/png".to_string(),
                    uri: None,
                    meta: None,
                }
            }]
        );
    }

    #[test]
    fn tool_call_updates_round_trip_full_wire_shape() {
        let path = PathBuf::from("src/main.rs");
        let path_json = serde_json::to_value(&path).expect("serialize path");
        let tool_call = AcpSessionUpdate::ToolCall {
            tool_call_id: "call-1".to_string(),
            title: "Read file".to_string(),
            kind: AcpToolKind::Read,
            status: AcpToolCallStatus::Pending,
            raw_input: Some(serde_json::json!({ "path": path_json.clone() })),
            raw_output: Some(serde_json::json!({ "ok": true })),
            content: vec![AcpToolCallContent::Content {
                content: AcpContentBlock::text("reading"),
            }],
            locations: vec![AcpToolCallLocation {
                path: path.clone(),
                line: Some(7),
            }],
        };
        let value = serde_json::to_value(&tool_call).expect("serialize tool call");
        assert_eq!(
            value,
            serde_json::json!({
                "sessionUpdate": "tool_call",
                "toolCallId": "call-1",
                "title": "Read file",
                "kind": "read",
                "status": "pending",
                "rawInput": { "path": path_json.clone() },
                "rawOutput": { "ok": true },
                "content": [
                    {
                        "type": "content",
                        "content": {
                            "type": "text",
                            "text": "reading"
                        }
                    }
                ],
                "locations": [
                    {
                        "path": path_json.clone(),
                        "line": 7
                    }
                ]
            })
        );
        assert_eq!(
            serde_json::from_value::<AcpSessionUpdate>(value).expect("deserialize tool call"),
            tool_call
        );

        let update = AcpSessionUpdate::ToolCallUpdate {
            tool_call_id: "call-1".to_string(),
            title: Some("Updated file".to_string()),
            kind: Some(AcpToolKind::Edit),
            status: Some(AcpToolCallStatus::Completed),
            raw_input: Some(serde_json::json!({ "path": path_json.clone() })),
            raw_output: Some(serde_json::json!({ "changed": true })),
            content: vec![
                AcpToolCallContent::Diff {
                    path: path.clone(),
                    old_text: Some("old\n".to_string()),
                    new_text: "new\n".to_string(),
                },
                AcpToolCallContent::Terminal {
                    terminal_id: "term_1".to_string(),
                },
            ],
            locations: vec![AcpToolCallLocation {
                path: path.clone(),
                line: None,
            }],
        };
        let value = serde_json::to_value(&update).expect("serialize tool call update");
        assert_eq!(
            value,
            serde_json::json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": "call-1",
                "title": "Updated file",
                "kind": "edit",
                "status": "completed",
                "rawInput": { "path": path_json.clone() },
                "rawOutput": { "changed": true },
                "content": [
                    {
                        "type": "diff",
                        "path": path_json.clone(),
                        "oldText": "old\n",
                        "newText": "new\n"
                    },
                    {
                        "type": "terminal",
                        "terminalId": "term_1"
                    }
                ],
                "locations": [
                    {
                        "path": path_json
                    }
                ]
            })
        );
        assert_eq!(
            serde_json::from_value::<AcpSessionUpdate>(value)
                .expect("deserialize tool call update"),
            update
        );
    }

    #[test]
    fn file_change_item_emits_acp_diff_content_and_locations() {
        let session_id = SessionId::new();
        let path = PathBuf::from("src/lib.rs");
        let raw_input = serde_json::json!({
            "path": serde_json::to_value(&path).expect("serialize path"),
        });
        let payload_value = serde_json::to_value(FileChangePayload {
            tool_call_id: "call-1".to_string(),
            tool_name: Some("apply_patch".to_string()),
            input: Some(raw_input.clone()),
            changes: vec![(
                path.clone(),
                crate::protocol::FileChange::Add {
                    content: "hello\n".to_string(),
                },
            )],
            is_error: false,
        })
        .expect("serialize file change payload");
        let event = ServerEvent::ItemCompleted(ItemEventPayload {
            context: EventContext {
                session_id,
                turn_id: Some(TurnId::new()),
                item_id: Some(ItemId::new()),
                seq: 1,
            },
            item: crate::ItemEnvelope {
                item_id: ItemId::new(),
                item_kind: ItemKind::FileChange,
                payload: payload_value.clone(),
            },
        });

        assert_eq!(
            acp_update_from_server_event(&event),
            Some(AcpSessionUpdate::ToolCallUpdate {
                tool_call_id: "call-1".to_string(),
                title: Some("apply_patch".to_string()),
                kind: Some(AcpToolKind::Edit),
                status: Some(AcpToolCallStatus::Completed),
                raw_input: Some(raw_input),
                raw_output: Some(payload_value),
                content: vec![AcpToolCallContent::Diff {
                    path: path.clone(),
                    old_text: None,
                    new_text: "hello\n".to_string(),
                }],
                locations: vec![AcpToolCallLocation { path, line: None }],
            })
        );
    }

    #[test]
    fn file_change_update_emits_text_content_when_old_new_text_is_unavailable() {
        let path = PathBuf::from("src/lib.rs");
        let unified_diff = "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let change = FileChangePayload {
            tool_call_id: "call-1".to_string(),
            tool_name: Some("apply_patch".to_string()),
            input: None,
            changes: vec![(
                path,
                crate::protocol::FileChange::Update {
                    unified_diff: unified_diff.to_string(),
                    move_path: None,
                },
            )],
            is_error: false,
        };

        assert_eq!(
            file_change_tool_content(&change),
            vec![AcpToolCallContent::Content {
                content: AcpContentBlock::text(unified_diff)
            }]
        );
    }

    #[test]
    fn request_permission_uses_acp_wire_shape() {
        let session_id = SessionId::new();
        let params = AcpRequestPermissionParams {
            session_id,
            tool_call: AcpToolCallUpdate {
                tool_call_id: "call-1".to_string(),
                title: Some("Run command".to_string()),
                kind: Some(AcpToolKind::Execute),
                status: Some(AcpToolCallStatus::Pending),
                raw_input: Some(serde_json::json!({"command": "cargo test"})),
                raw_output: None,
                content: Vec::new(),
                locations: Vec::new(),
                meta: None,
            },
            options: vec![AcpPermissionOption {
                option_id: "allow_once".to_string(),
                name: "Allow once".to_string(),
                kind: AcpPermissionOptionKind::AllowOnce,
                meta: None,
            }],
            meta: None,
        };

        assert_eq!(
            serde_json::to_value(params).expect("serialize permission request"),
            serde_json::json!({
                "sessionId": session_id,
                "toolCall": {
                    "toolCallId": "call-1",
                    "title": "Run command",
                    "kind": "execute",
                    "status": "pending",
                    "rawInput": { "command": "cargo test" }
                },
                "options": [
                    {
                        "optionId": "allow_once",
                        "name": "Allow once",
                        "kind": "allow_once"
                    }
                ]
            })
        );

        let response: AcpRequestPermissionResponse = serde_json::from_value(serde_json::json!({
            "outcome": {
                "outcome": "selected",
                "optionId": "allow_once"
            }
        }))
        .expect("deserialize permission response");
        assert_eq!(
            response,
            AcpRequestPermissionResponse {
                outcome: AcpPermissionOutcome::Selected {
                    option_id: "allow_once".to_string()
                },
                meta: None,
            }
        );
    }

    #[test]
    fn usage_update_size_uses_context_window() {
        let session_id = SessionId::new();
        let event = ServerEvent::TurnUsageUpdated(crate::TurnUsageUpdatedPayload {
            session_id,
            turn_id: TurnId::new(),
            usage: crate::TurnUsage {
                input_tokens: 3,
                output_tokens: 4,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            },
            total_input_tokens: 30,
            total_output_tokens: 12,
            total_cache_read_tokens: 0,
            last_query_input_tokens: 3,
            context_window: Some(200_000),
        });

        let (_, value) = acp_notification_from_server_event("turn/usage/updated", &event);

        assert_eq!(
            value["update"],
            serde_json::json!({
                "sessionUpdate": "usage_update",
                "used": 42,
                "size": 200000
            })
        );
    }

    #[test]
    fn tool_status_maps_pending_then_in_progress_update() {
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let item_id = ItemId::new();
        let started = ServerEvent::ItemStarted(ItemEventPayload {
            context: EventContext {
                session_id,
                turn_id: Some(turn_id),
                item_id: Some(item_id),
                seq: 0,
            },
            item: crate::ItemEnvelope {
                item_id,
                item_kind: ItemKind::ToolCall,
                payload: serde_json::to_value(ToolCallPayload {
                    tool_call_id: "call-1".to_string(),
                    tool_name: "read".to_string(),
                    parameters: serde_json::json!({"path": "src/lib.rs"}),
                    command_actions: Vec::new(),
                })
                .expect("serialize tool payload"),
            },
        });
        let (_, started_value) = acp_notification_from_server_event("item/started", &started);

        assert_eq!(
            started_value["update"]["status"],
            serde_json::json!("pending")
        );

        let update = ServerEvent::ToolCallStatusUpdated(crate::ToolCallStatusUpdatedPayload {
            session_id,
            turn_id,
            tool_call_id: "call-1".to_string(),
            status: "in_progress".to_string(),
        });
        let (_, update_value) =
            acp_notification_from_server_event("tool_call/status_updated", &update);

        assert_eq!(
            update_value["update"],
            serde_json::json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": "call-1",
                "status": "in_progress"
            })
        );
    }

    #[test]
    fn session_update_preserves_devo_event_in_meta() {
        let session_id = SessionId::new();
        let item_id = ItemId::new();
        let event = ServerEvent::ItemDelta {
            delta_kind: ItemDeltaKind::AgentMessageDelta,
            payload: ItemDeltaPayload {
                context: EventContext {
                    session_id,
                    turn_id: None,
                    item_id: Some(item_id),
                    seq: 7,
                },
                delta: "hello".to_string(),
                stream_index: None,
                channel: None,
            },
        };

        let (method, value) = acp_notification_from_server_event("item/agentMessage/delta", &event);
        let notification: AcpSessionNotification =
            serde_json::from_value(value.clone()).expect("deserialize ACP notification");

        assert_eq!(method, ACP_SESSION_UPDATE_METHOD);
        assert_eq!(
            value["update"],
            serde_json::json!({
                "sessionUpdate": "agent_message_chunk",
                "content": {
                    "type": "text",
                    "text": "hello"
                },
                "messageId": item_id.to_string()
            })
        );
        assert_eq!(
            original_event_from_acp_notification(&notification),
            Some(("item/agentMessage/delta".to_string(), event))
        );
    }
}

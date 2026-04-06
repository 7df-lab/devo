use std::path::PathBuf;
use std::sync::Arc;
use std::{collections::HashMap, sync::RwLock};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

/// Strongly typed tool name used by the spec-aligned tool subsystem.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolName(
    /// The stable string value of the tool name.
    pub SmolStr,
);

/// Strongly typed identifier for one tool invocation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolCallId(
    /// The stable UUID-backed string for the tool invocation.
    pub String,
);

/// Describes one model-visible tool definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDefinitionSpec {
    /// The stable runtime tool name.
    pub name: ToolName,
    /// The human-readable tool description.
    pub description: String,
    /// The JSON schema describing the expected input shape.
    pub input_schema: serde_json::Value,
    /// The output mode exposed to the runtime and model.
    pub output_mode: ToolOutputMode,
    /// Capability tags describing the resources touched by the tool.
    pub capability_tags: Vec<ToolCapabilityTag>,
    /// The approval hint surfaced before execution.
    pub needs_approval: ApprovalHint,
}

/// Describes the output shape returned by a tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolOutputMode {
    /// The tool returns structured JSON only.
    StructuredJson,
    /// The tool returns text only.
    Text,
    /// The tool may return both text and JSON.
    Mixed,
}

/// Tags one tool with the resources it may touch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolCapabilityTag {
    /// The tool reads files.
    ReadFiles,
    /// The tool writes files.
    WriteFiles,
    /// The tool executes a subprocess.
    ExecuteProcess,
    /// The tool accesses the network.
    NetworkAccess,
    /// The tool searches the workspace.
    SearchWorkspace,
    /// The tool reads images.
    ReadImages,
}

/// Describes whether approval is never, maybe, or always expected for a tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalHint {
    /// The tool should never need approval for normal execution.
    Never,
    /// The tool may need approval depending on input and policy.
    Maybe,
    /// The tool should always go through approval flow.
    Always,
}

/// Stores one normalized tool invocation request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolInvocation {
    /// The stable tool-call identifier.
    pub tool_call_id: ToolCallId,
    /// The session that owns the invocation.
    pub session_id: String,
    /// The turn that owns the invocation.
    pub turn_id: String,
    /// The requested tool name.
    pub tool_name: ToolName,
    /// The validated JSON input payload.
    pub input: serde_json::Value,
    /// The request timestamp.
    pub requested_at: DateTime<Utc>,
}

/// Carries the execution context passed to a tool implementation.
#[derive(Debug, Clone)]
pub struct ToolExecutionContext {
    /// The session that owns the invocation.
    pub session_id: String,
    /// The turn that owns the invocation.
    pub turn_id: String,
    /// The current working directory for execution.
    pub cwd: PathBuf,
    /// The safety snapshot active for this invocation.
    pub policy_snapshot: ToolPolicySnapshot,
    /// The normalized application config visible to the tool runtime.
    pub app_config: Arc<ToolRuntimeConfigSnapshot>,
}

/// Stores the safety-policy snapshot visible to one tool execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolPolicySnapshot {
    /// The stable policy mode label active for the execution.
    pub mode: String,
    /// The human-readable policy summary visible to logs or debugging.
    pub summary: Option<String>,
}

impl ToolExecutionContext {
    /// Returns the enabled tool runtime config from the application config snapshot.
    pub fn tool_runtime_config(&self) -> &ToolRuntimeConfigSnapshot {
        &self.app_config
    }
}

/// Stores the tool-runtime config snapshot visible to one tool execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRuntimeConfigSnapshot {
    /// The set of tool names enabled by the enclosing application config.
    pub enabled_tools: Vec<String>,
    /// The normalized shell-command runtime config.
    pub shell: ShellToolConfigSnapshot,
    /// The normalized file-search runtime config.
    pub file_search: FileSearchToolConfigSnapshot,
    /// The maximum number of read-only tools that may run concurrently.
    pub max_parallel_read_tools: u16,
}

/// Stores the shell-command tool config snapshot visible to the runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellToolConfigSnapshot {
    /// The default timeout applied when a request does not specify one.
    pub default_timeout_ms: u64,
    /// The maximum timeout allowed for one shell command request.
    pub max_timeout_ms: u64,
    /// Whether stdout and stderr streaming should be emitted incrementally.
    pub stream_output: bool,
    /// The maximum number of stdout bytes preserved in the terminal payload.
    pub max_stdout_bytes: usize,
    /// The maximum number of stderr bytes preserved in the terminal payload.
    pub max_stderr_bytes: usize,
}

/// Stores the file-search tool config snapshot visible to the runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSearchToolConfigSnapshot {
    /// Whether the backend should prefer `rg` when available.
    pub prefer_rg: bool,
    /// The maximum number of matches preserved in one result payload.
    pub max_results: u32,
    /// The maximum preview size preserved for one match.
    pub max_preview_bytes: usize,
}

/// Describes the normalized terminal outcome of a tool execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolExecutionOutcome {
    /// The tool completed successfully.
    Completed(ToolResultPayload),
    /// The tool failed during execution.
    Failed(ToolFailure),
    /// The tool was denied by policy or approval flow.
    Denied(ToolDenied),
    /// The tool was interrupted before completion.
    Interrupted,
}

/// Stores the normalized successful payload returned by a tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResultPayload {
    /// The result content returned by the tool.
    pub content: ToolContent,
    /// Structured execution metadata.
    pub metadata: ToolResultMetadata,
}

/// Stores the content returned by a tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolContent {
    /// Plain text content.
    Text(String),
    /// Structured JSON content.
    Json(serde_json::Value),
    /// Mixed text and JSON content.
    Mixed {
        /// Optional text content.
        text: Option<String>,
        /// Optional structured JSON content.
        json: Option<serde_json::Value>,
    },
}

/// Stores structured metadata for a tool result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolResultMetadata {
    /// Whether the output was truncated before persistence or model reuse.
    pub truncated: bool,
    /// Optional execution duration in milliseconds.
    pub duration_ms: Option<u64>,
}

/// Stores a normalized terminal failure for a tool execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolFailure {
    /// The stable machine-readable error code.
    pub code: String,
    /// The human-readable failure message.
    pub message: String,
}

/// Stores a normalized policy denial for a tool execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDenied {
    /// The human-readable denial reason.
    pub reason: String,
}

/// Reports incremental progress from a running tool.
pub trait ToolProgressReporter: Send + Sync {
    /// Emits one human-readable progress message.
    fn report(&self, message: &str);
}

/// Spec-aligned tool contract used by the newer runtime layers.
#[async_trait]
pub trait RuntimeTool: Send + Sync {
    /// Returns the model-visible tool definition.
    fn definition(&self) -> ToolDefinitionSpec;

    /// Validates a candidate tool input before approval or execution.
    async fn validate(&self, input: &serde_json::Value) -> Result<(), ToolInputError>;

    /// Executes the tool against the normalized execution context.
    async fn execute(
        &self,
        input: serde_json::Value,
        ctx: ToolExecutionContext,
        reporter: Arc<dyn ToolProgressReporter>,
    ) -> Result<ToolExecutionOutcome, ToolExecuteError>;
}

/// Spec-aligned registry contract used by the newer runtime layers.
pub trait RuntimeToolRegistry: Send + Sync {
    /// Returns one runtime tool by name.
    fn get(&self, name: &ToolName) -> Option<Arc<dyn RuntimeTool>>;

    /// Lists the model-visible tool definitions.
    fn list(&self) -> Vec<ToolDefinitionSpec>;
}

/// In-memory implementation of the spec-aligned tool registry.
pub struct InMemoryRuntimeToolRegistry {
    /// The registered runtime tools keyed by their stable tool names.
    tools: RwLock<HashMap<ToolName, Arc<dyn RuntimeTool>>>,
}

impl InMemoryRuntimeToolRegistry {
    /// Creates an empty in-memory runtime tool registry.
    pub fn new() -> Self {
        Self {
            tools: RwLock::new(HashMap::new()),
        }
    }

    /// Registers or replaces one runtime tool definition by its stable name.
    pub fn register(&self, tool: Arc<dyn RuntimeTool>) {
        let definition = tool.definition();
        self.tools
            .write()
            .expect("runtime tool registry poisoned")
            .insert(definition.name, tool);
    }

    /// Returns whether one named tool is enabled by app configuration.
    pub fn is_enabled(&self, app_config: &ToolRuntimeConfigSnapshot, name: &ToolName) -> bool {
        app_config
            .enabled_tools
            .iter()
            .any(|enabled| enabled == name.0.as_str())
    }

    /// Lists the enabled tool definitions visible to the current application config.
    pub fn list_enabled(&self, app_config: &ToolRuntimeConfigSnapshot) -> Vec<ToolDefinitionSpec> {
        self.tools
            .read()
            .expect("runtime tool registry poisoned")
            .values()
            .filter_map(|tool| {
                let definition = tool.definition();
                self.is_enabled(app_config, &definition.name)
                    .then_some(definition)
            })
            .collect()
    }
}

impl Default for InMemoryRuntimeToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeToolRegistry for InMemoryRuntimeToolRegistry {
    fn get(&self, name: &ToolName) -> Option<Arc<dyn RuntimeTool>> {
        self.tools
            .read()
            .expect("runtime tool registry poisoned")
            .get(name)
            .cloned()
    }

    fn list(&self) -> Vec<ToolDefinitionSpec> {
        self.tools
            .read()
            .expect("runtime tool registry poisoned")
            .values()
            .map(|tool| tool.definition())
            .collect()
    }
}

/// No-op reporter used when a caller does not need incremental tool progress.
pub struct NullToolProgressReporter;

impl ToolProgressReporter for NullToolProgressReporter {
    fn report(&self, _message: &str) {}
}

/// Stores the spec-defined shell command input payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellCommandInput {
    /// The shell command string to execute through the configured shell adapter.
    pub command: String,
    /// The optional working directory override for the subprocess.
    pub workdir: Option<PathBuf>,
    /// The optional execution timeout in milliseconds.
    pub timeout_ms: Option<u64>,
    /// The optional environment-variable overrides to inject into the subprocess.
    pub environment: Option<std::collections::BTreeMap<String, String>>,
    /// The optional execution-escalation request metadata.
    pub escalation: Option<ShellEscalationRequest>,
}

/// Stores the spec-defined shell escalation request metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellEscalationRequest {
    /// The human-readable justification shown during approval.
    pub justification: String,
    /// The requested sandbox mode for the escalated execution.
    pub sandbox_permissions: String,
    /// The optional command-prefix allow rule suggested by the caller.
    pub prefix_rule: Option<Vec<String>>,
}

/// Stores the normalized shell command result payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellCommandResult {
    /// The optional process exit code.
    pub exit_code: Option<i32>,
    /// Captured standard output text.
    pub stdout: String,
    /// Captured standard error text.
    pub stderr: String,
    /// Total wall-clock execution time in milliseconds.
    pub duration_ms: u64,
    /// Whether the subprocess hit the configured timeout.
    pub timed_out: bool,
    /// Whether standard output was truncated.
    pub truncated_stdout: bool,
    /// Whether standard error was truncated.
    pub truncated_stderr: bool,
}

/// Stores the spec-defined file search input payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSearchInput {
    /// The literal or regex-like query string for the backend search implementation.
    pub query: String,
    /// The search mode requested by the caller.
    pub mode: FileSearchMode,
    /// The optional workspace roots to search inside.
    pub roots: Option<Vec<PathBuf>>,
    /// The optional glob filters used to restrict candidate files.
    pub glob: Option<Vec<String>>,
    /// Whether matching should be case-sensitive.
    pub case_sensitive: bool,
    /// The optional maximum number of matches requested by the caller.
    pub max_results: Option<u32>,
}

/// Selects the backend search mode for a file search request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileSearchMode {
    /// Search file contents.
    Content,
    /// Search file names or paths.
    FileName,
    /// Let the backend choose the mode heuristically.
    Auto,
}

/// Stores the normalized file search result payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSearchResult {
    /// The effective search mode used for the completed search.
    pub mode: FileSearchMode,
    /// The bounded normalized search matches.
    pub matches: Vec<FileSearchMatch>,
    /// Whether the backend had to truncate the result list.
    pub truncated: bool,
}

/// Stores one normalized file search match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSearchMatch {
    /// The stable file path for the match.
    pub path: PathBuf,
    /// The optional one-based line number for content matches.
    pub line: Option<u32>,
    /// The optional one-based column number for content matches.
    pub column: Option<u32>,
    /// The bounded preview text for the match.
    pub preview: String,
}

/// Describes failures during tool input validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum ToolInputError {
    /// The input payload failed schema or semantic validation.
    #[error("invalid tool input: {message}")]
    Invalid { message: String },
}

/// Describes normalized failures produced by the tool runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum ToolExecuteError {
    /// The requested tool name was unknown.
    #[error("unknown tool: {tool_name}")]
    UnknownTool { tool_name: String },
    /// The input payload was invalid.
    #[error("invalid input: {message}")]
    InvalidInput { message: String },
    /// Approval was required before the tool could proceed.
    #[error("approval required: {message}")]
    ApprovalRequired { message: String },
    /// The request was denied by safety policy.
    #[error("permission denied: {message}")]
    PermissionDenied { message: String },
    /// No sandbox was available to execute the tool.
    #[error("sandbox unavailable: {message}")]
    SandboxUnavailable { message: String },
    /// Backend execution failed.
    #[error("execution failed: {message}")]
    ExecutionFailed { message: String },
    /// The tool timed out.
    #[error("timeout: {message}")]
    Timeout { message: String },
    /// The tool was interrupted.
    #[error("interrupted: {message}")]
    Interrupted { message: String },
    /// An internal invariant failed.
    #[error("internal tool error: {message}")]
    Internal { message: String },
}

#[cfg(test)]
mod tests {
    use super::{
        ApprovalHint, FileSearchMode, InMemoryRuntimeToolRegistry, NullToolProgressReporter,
        RuntimeToolRegistry, ToolCapabilityTag, ToolContent, ToolDefinitionSpec, ToolName,
        ToolProgressReporter, ToolResultMetadata, ToolResultPayload,
    };

    #[test]
    fn tool_definition_roundtrip() {
        let definition = ToolDefinitionSpec {
            name: ToolName("shell_command".into()),
            description: "Run a shell command".into(),
            input_schema: serde_json::json!({"type":"object"}),
            output_mode: super::ToolOutputMode::Text,
            capability_tags: vec![ToolCapabilityTag::ExecuteProcess],
            needs_approval: ApprovalHint::Maybe,
        };

        let json = serde_json::to_string(&definition).expect("serialize");
        let restored: ToolDefinitionSpec = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(definition, restored);
    }

    #[test]
    fn result_payload_supports_mixed_content() {
        let payload = ToolResultPayload {
            content: ToolContent::Mixed {
                text: Some("done".into()),
                json: Some(serde_json::json!({"ok":true})),
            },
            metadata: ToolResultMetadata {
                truncated: false,
                duration_ms: Some(15),
            },
        };

        let json = serde_json::to_string(&payload).expect("serialize");
        let restored: ToolResultPayload = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(payload, restored);
    }

    #[test]
    fn runtime_registry_starts_empty() {
        let registry = InMemoryRuntimeToolRegistry::new();
        assert!(registry.list().is_empty());
    }

    #[test]
    fn file_search_mode_roundtrip() {
        let json = serde_json::to_string(&FileSearchMode::Auto).expect("serialize");
        let restored: FileSearchMode = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored, FileSearchMode::Auto);
    }

    #[test]
    fn null_reporter_accepts_progress() {
        let reporter = NullToolProgressReporter;
        reporter.report("working");
    }
}

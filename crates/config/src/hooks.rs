use std::collections::BTreeMap;

use serde::Deserialize;
use serde::Serialize;

/// External hook events understood by Devo configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum HookEvent {
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
    Notification,
    UserPromptSubmit,
    SessionStart,
    SessionEnd,
    Stop,
    StopFailure,
    SubagentStart,
    SubagentStop,
    PreCompact,
    PostCompact,
    PermissionRequest,
    PermissionDenied,
    Setup,
    TeammateIdle,
    TaskCreated,
    TaskCompleted,
    Elicitation,
    ElicitationResult,
    ConfigChange,
    WorktreeCreate,
    WorktreeRemove,
    InstructionsLoaded,
    CwdChanged,
    FileChanged,
}

/// Top-level hook configuration keyed by event name.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HooksConfig(pub BTreeMap<HookEvent, Vec<HookMatcherConfig>>);

impl HooksConfig {
    pub fn is_empty(&self) -> bool {
        self.0.values().all(Vec::is_empty)
    }

    pub fn matchers_for(&self, event: HookEvent) -> &[HookMatcherConfig] {
        self.0.get(&event).map_or(&[], Vec::as_slice)
    }
}

/// A matcher groups one or more hooks for an event-specific match query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookMatcherConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hooks: Vec<HookCommandConfig>,
}

/// Persistable hook command definitions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HookCommandConfig {
    Command(CommandHookConfig),
    Prompt(PromptHookConfig),
    Agent(AgentHookConfig),
    Http(HttpHookConfig),
}

/// Shell command hook definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandHookConfig {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<HookShell>,
    #[serde(rename = "if", default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
    #[serde(
        rename = "statusMessage",
        alias = "status_message",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub status_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub once: Option<bool>,
    #[serde(rename = "async", default, skip_serializing_if = "Option::is_none")]
    pub async_hook: Option<bool>,
    #[serde(
        rename = "asyncRewake",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub async_rewake: Option<bool>,
}

/// Shell used to run a command hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookShell {
    #[serde(rename = "bash")]
    Bash,
    #[serde(rename = "powershell", alias = "power_shell")]
    PowerShell,
}

/// Parsed but currently unsupported LLM prompt hook definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptHookConfig {
    pub prompt: String,
    #[serde(rename = "if", default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(
        rename = "statusMessage",
        alias = "status_message",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub status_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub once: Option<bool>,
}

/// Parsed but currently unsupported agent hook definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentHookConfig {
    pub prompt: String,
    #[serde(rename = "if", default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(
        rename = "statusMessage",
        alias = "status_message",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub status_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub once: Option<bool>,
}

/// Parsed but currently unsupported HTTP hook definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpHookConfig {
    pub url: String,
    #[serde(rename = "if", default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(
        rename = "allowedEnvVars",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub allowed_env_vars: Vec<String>,
    #[serde(
        rename = "statusMessage",
        alias = "status_message",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub status_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub once: Option<bool>,
}

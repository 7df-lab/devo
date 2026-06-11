use std::collections::HashSet;
mod command;

use std::path::PathBuf;
use std::sync::Arc;

use devo_config::HookCommandConfig;
use devo_config::HookEvent;
use devo_config::HooksConfig;
use serde_json::Map;
use serde_json::Value;
use tracing::warn;

#[derive(Debug, Clone)]
pub struct HookRunner {
    config: Arc<HooksConfig>,
}

impl HookRunner {
    pub fn new(config: HooksConfig) -> Self {
        Self {
            config: Arc::new(config),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.config.is_empty()
    }

    pub async fn run(&self, input: HookInput) -> HookRunReport {
        let match_query = input.match_query();
        let mut report = HookRunReport::default();
        let mut seen = HashSet::new();

        for matcher in self.config.matchers_for(input.event()) {
            if let Some(matcher_text) = matcher.matcher.as_deref()
                && let Some(query) = match_query.as_deref()
                && !matches_pattern(query, matcher_text)
            {
                continue;
            }

            for hook in &matcher.hooks {
                match hook {
                    HookCommandConfig::Command(command) => {
                        let key = command::command_dedup_key(command);
                        if !seen.insert(key) {
                            continue;
                        }
                        let result = command::execute_command_hook(command, input.payload()).await;
                        report.push(result);
                    }
                    HookCommandConfig::Prompt(_)
                    | HookCommandConfig::Agent(_)
                    | HookCommandConfig::Http(_) => {
                        report.unsupported += 1;
                        warn!(
                            event = ?input.event(),
                            hook_type = hook_type(hook),
                            "hook type is parsed but not supported by the current runtime"
                        );
                    }
                }
            }
        }

        report
    }
}

#[derive(Debug, Clone)]
pub struct HookRuntimeContext {
    pub runner: HookRunner,
    pub base: HookBaseInput,
}

#[derive(Debug, Clone)]
pub struct HookBaseInput {
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: PathBuf,
    pub permission_mode: Option<String>,
    pub agent_id: Option<String>,
    pub agent_type: Option<String>,
}

#[derive(Debug, Clone)]
pub struct HookInput {
    event: HookEvent,
    payload: Value,
}

impl HookInput {
    pub fn new(base: &HookBaseInput, event: HookEvent, extra: Map<String, Value>) -> Self {
        let mut payload = Map::new();
        payload.insert(
            "hook_event_name".to_string(),
            Value::String(hook_event_name(event).to_string()),
        );
        payload.insert(
            "session_id".to_string(),
            Value::String(base.session_id.clone()),
        );
        payload.insert(
            "transcript_path".to_string(),
            Value::String(base.transcript_path.clone()),
        );
        payload.insert(
            "cwd".to_string(),
            Value::String(base.cwd.display().to_string()),
        );
        if let Some(permission_mode) = &base.permission_mode {
            payload.insert(
                "permission_mode".to_string(),
                Value::String(permission_mode.clone()),
            );
        }
        if let Some(agent_id) = &base.agent_id {
            payload.insert("agent_id".to_string(), Value::String(agent_id.clone()));
        }
        if let Some(agent_type) = &base.agent_type {
            payload.insert("agent_type".to_string(), Value::String(agent_type.clone()));
        }
        payload.extend(extra);
        Self {
            event,
            payload: Value::Object(payload),
        }
    }

    pub fn event(&self) -> HookEvent {
        self.event
    }

    pub fn payload(&self) -> &Value {
        &self.payload
    }

    fn match_query(&self) -> Option<String> {
        let object = self.payload.as_object()?;
        let field = match self.event {
            HookEvent::PreToolUse
            | HookEvent::PostToolUse
            | HookEvent::PostToolUseFailure
            | HookEvent::PermissionRequest
            | HookEvent::PermissionDenied => "tool_name",
            HookEvent::SessionStart => "source",
            HookEvent::SessionEnd => "reason",
            HookEvent::Setup => "trigger",
            HookEvent::PreCompact | HookEvent::PostCompact => "trigger",
            HookEvent::Notification => "notification_type",
            HookEvent::StopFailure => "error",
            HookEvent::SubagentStart | HookEvent::SubagentStop => "agent_type",
            HookEvent::Elicitation | HookEvent::ElicitationResult => "mcp_server_name",
            HookEvent::ConfigChange => "source",
            HookEvent::InstructionsLoaded => "load_reason",
            HookEvent::FileChanged => return object.get("file_path").and_then(file_name_query),
            HookEvent::UserPromptSubmit
            | HookEvent::Stop
            | HookEvent::TeammateIdle
            | HookEvent::TaskCreated
            | HookEvent::TaskCompleted
            | HookEvent::WorktreeCreate
            | HookEvent::WorktreeRemove
            | HookEvent::CwdChanged => return None,
        };
        object
            .get(field)
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookCommandResult {
    pub command: String,
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub outcome: HookCommandOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookCommandOutcome {
    Success,
    Blocking { reason: String },
    NonBlockingError { message: String },
    Spawned,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookRunReport {
    pub results: Vec<HookCommandResult>,
    pub unsupported: usize,
}

impl HookRunReport {
    pub fn first_blocking_reason(&self) -> Option<&str> {
        self.results
            .iter()
            .find_map(|result| match &result.outcome {
                HookCommandOutcome::Blocking { reason } => Some(reason.as_str()),
                HookCommandOutcome::Success
                | HookCommandOutcome::NonBlockingError { .. }
                | HookCommandOutcome::Spawned => None,
            })
    }

    fn push(&mut self, result: HookCommandResult) {
        if let HookCommandOutcome::NonBlockingError { message } = &result.outcome {
            warn!(
                command = %result.command,
                status = ?result.status,
                error = %message,
                "hook command failed without blocking"
            );
        }
        self.results.push(result);
    }
}

fn hook_type(hook: &HookCommandConfig) -> &'static str {
    match hook {
        HookCommandConfig::Command(_) => "command",
        HookCommandConfig::Prompt(_) => "prompt",
        HookCommandConfig::Agent(_) => "agent",
        HookCommandConfig::Http(_) => "http",
    }
}

fn hook_event_name(event: HookEvent) -> &'static str {
    match event {
        HookEvent::PreToolUse => "PreToolUse",
        HookEvent::PostToolUse => "PostToolUse",
        HookEvent::PostToolUseFailure => "PostToolUseFailure",
        HookEvent::Notification => "Notification",
        HookEvent::UserPromptSubmit => "UserPromptSubmit",
        HookEvent::SessionStart => "SessionStart",
        HookEvent::SessionEnd => "SessionEnd",
        HookEvent::Stop => "Stop",
        HookEvent::StopFailure => "StopFailure",
        HookEvent::SubagentStart => "SubagentStart",
        HookEvent::SubagentStop => "SubagentStop",
        HookEvent::PreCompact => "PreCompact",
        HookEvent::PostCompact => "PostCompact",
        HookEvent::PermissionRequest => "PermissionRequest",
        HookEvent::PermissionDenied => "PermissionDenied",
        HookEvent::Setup => "Setup",
        HookEvent::TeammateIdle => "TeammateIdle",
        HookEvent::TaskCreated => "TaskCreated",
        HookEvent::TaskCompleted => "TaskCompleted",
        HookEvent::Elicitation => "Elicitation",
        HookEvent::ElicitationResult => "ElicitationResult",
        HookEvent::ConfigChange => "ConfigChange",
        HookEvent::WorktreeCreate => "WorktreeCreate",
        HookEvent::WorktreeRemove => "WorktreeRemove",
        HookEvent::InstructionsLoaded => "InstructionsLoaded",
        HookEvent::CwdChanged => "CwdChanged",
        HookEvent::FileChanged => "FileChanged",
    }
}

fn file_name_query(value: &Value) -> Option<String> {
    let path = value.as_str()?;
    std::path::Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
}

fn matches_pattern(query: &str, matcher: &str) -> bool {
    if matcher.is_empty() || matcher == "*" {
        return true;
    }
    if matcher
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '|')
    {
        return matcher.split('|').any(|candidate| candidate == query);
    }
    regex::Regex::new(matcher).is_ok_and(|regex| regex.is_match(query))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use devo_config::CommandHookConfig;
    use devo_config::HookCommandConfig;
    use devo_config::HookMatcherConfig;
    use pretty_assertions::assert_eq;

    use super::*;

    #[tokio::test]
    async fn command_hook_receives_json_on_stdin() {
        let config = HooksConfig(BTreeMap::from([(
            HookEvent::PreToolUse,
            vec![HookMatcherConfig {
                matcher: Some("exec_command".to_string()),
                hooks: vec![HookCommandConfig::Command(CommandHookConfig {
                    command: "node -e \"let s='';process.stdin.on('data',d=>s+=d);process.stdin.on('end',()=>{const j=JSON.parse(s);process.stdout.write(j.tool_name)})\"".to_string(),
                    shell: None,
                    condition: None,
                    timeout: Some(5),
                    status_message: None,
                    once: None,
                    async_hook: None,
                    async_rewake: None,
                })],
            }],
        )]));
        let runner = HookRunner::new(config);
        let base = HookBaseInput {
            session_id: "session".to_string(),
            transcript_path: "rollout.jsonl".to_string(),
            cwd: std::env::current_dir().expect("current dir"),
            permission_mode: Some("interactive".to_string()),
            agent_id: None,
            agent_type: None,
        };
        let input = HookInput::new(
            &base,
            HookEvent::PreToolUse,
            Map::from_iter([
                (
                    "tool_name".to_string(),
                    Value::String("exec_command".to_string()),
                ),
                ("tool_input".to_string(), Value::Object(Map::new())),
                (
                    "tool_use_id".to_string(),
                    Value::String("call-1".to_string()),
                ),
            ]),
        );

        let report = runner.run(input).await;

        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].stdout, "exec_command");
        assert_eq!(report.results[0].outcome, HookCommandOutcome::Success);
    }

    #[test]
    fn matcher_supports_pipe_and_regex() {
        assert!(matches_pattern("read", "read|write"));
        assert!(matches_pattern("exec_command", "^exec_.*"));
        assert!(!matches_pattern("read", "write"));
    }
}

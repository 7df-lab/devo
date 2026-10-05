use std::collections::BTreeMap;

use crate::handler_kind::ToolHandlerKind;
use crate::json_schema::JsonSchema;
use crate::tool_spec::{
    ToolCapabilityTag, ToolExecutionMode, ToolOutputMode, ToolPreparationFeedback, ToolSpec,
};
use crate::tools::websearch_prompt::web_search_prompt;
use devo_config::AppConfig;

const SHELL_COMMAND_DESCRIPTION: &str = include_str!("shell_command.txt");
const WEBFETCH_DESCRIPTION: &str = include_str!("webfetch.txt");
const APPLY_PATCH_DESCRIPTION: &str = include_str!("apply_patch.txt");

#[derive(Debug, Clone)]
pub struct ToolRegistryPlan {
    pub specs: Vec<ToolSpec>,
    pub handlers: Vec<(ToolHandlerKind, String)>,
}

impl ToolRegistryPlan {
    pub fn new() -> Self {
        ToolRegistryPlan {
            specs: Vec::new(),
            handlers: Vec::new(),
        }
    }

    fn push(&mut self, spec: ToolSpec, kind: ToolHandlerKind) {
        let name = spec.name.clone();
        self.specs.push(spec);
        self.handlers.push((kind, name));
    }
}

impl Default for ToolRegistryPlan {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPlanConfig {
    pub use_shell_command: bool,
    pub use_unified_exec: bool,
    pub web_search: bool,
    pub web_fetch: bool,
    pub network_proxy: Option<String>,
    pub network_no_proxy: Option<String>,
    /// Internal registry execution surface. Model requests separately expose
    /// only the Python kernel schema.
    pub execution_surface: devo_kernel::ExecutionSurface,
}

impl ToolPlanConfig {
    pub fn from_app_config(config: &AppConfig) -> Self {
        Self {
            web_search: app_config_uses_local_web_search(config),
            web_fetch: app_config_uses_local_web_fetch(config),
            network_proxy: config.provider_http.proxy_url.clone(),
            network_no_proxy: config.provider_http.no_proxy.clone(),
            execution_surface: match config.tools.execution_surface {
                devo_config::ToolExecutionSurface::Rlm => devo_kernel::ExecutionSurface::Rlm,
                devo_config::ToolExecutionSurface::Discrete => {
                    devo_kernel::ExecutionSurface::Discrete
                }
            },
            ..Self::default()
        }
    }

    pub fn validate(&self) {
        // No incompatible combinations currently exist.
        // - use_shell_command and use_unified_exec are independent (shell_command is the
        //   canonical shell tool name; setting use_shell_command false keeps legacy "bash")
        // - unified exec adds new tools alongside shell_command
        // - all can be true simultaneously with no conflict
    }
}

impl Default for ToolPlanConfig {
    fn default() -> Self {
        ToolPlanConfig {
            use_shell_command: true,
            use_unified_exec: true,
            web_search: false,
            web_fetch: true,
            network_proxy: None,
            network_no_proxy: None,
            // Internal plan default. Application config and a live-kernel turn
            // select Rlm explicitly; model requests expose only ipython.
            execution_surface: devo_kernel::ExecutionSurface::Discrete,
        }
    }
}

/// Shared ToolSpec for the shell tool (`shell_command`, or legacy name `bash`).
pub(crate) fn shell_command_tool_spec(name: impl Into<String>) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: shell_command_description(),
        input_schema: shell_command_schema(),
        output_mode: ToolOutputMode::Mixed,
        execution_mode: ToolExecutionMode::Mutating,
        capability_tags: vec![ToolCapabilityTag::ExecuteProcess],
        supports_parallel: false,
        preparation_feedback: ToolPreparationFeedback::None,
        display_name: None,
        supports_cancellation: None,
        supports_streaming: None,
    }
}

fn shell_command_schema() -> JsonSchema {
    JsonSchema::object(
        BTreeMap::from([
            (
                "command".to_string(),
                JsonSchema::string(Some(
                    "The shell command to execute in the selected platform shell.",
                )),
            ),
            (
                "cmd".to_string(),
                JsonSchema::string(Some("Alias for command")),
            ),
            (
                "timeout".to_string(),
                JsonSchema::integer(Some("Optional timeout in milliseconds")),
            ),
            (
                "timeout_ms".to_string(),
                JsonSchema::integer(Some("Alias for timeout")),
            ),
            (
                "workdir".to_string(),
                JsonSchema::string(Some(
                    "The working directory to run the command in. Defaults to the current directory. Use this instead of 'cd' commands.",
                )),
            ),
            (
                "description".to_string(),
                JsonSchema::string(Some(
                    "Clear, concise description of what this command does in 5-10 words.",
                )),
            ),
            (
                "shell".to_string(),
                JsonSchema::string(Some(
                    "Optional shell binary to launch. Defaults to the user's default shell.",
                )),
            ),
            (
                "tty".to_string(),
                JsonSchema::boolean(Some(
                    "Whether to allocate a TTY for the command. Defaults to false.",
                )),
            ),
            (
                "login".to_string(),
                JsonSchema::boolean(Some(
                    "Whether to run the shell with login shell semantics. Defaults to true.",
                )),
            ),
            (
                "yield_time_ms".to_string(),
                JsonSchema::number(Some(
                    "How long to wait (in milliseconds) for output before yielding.",
                )),
            ),
            (
                "max_output_tokens".to_string(),
                JsonSchema::number(Some(
                    "Maximum number of tokens to return. Excess output will be truncated.",
                )),
            ),
        ]),
        Some(vec!["command".to_string()]),
        Some(false),
    )
}

fn shell_command_description() -> String {
    let chaining = if cfg!(windows) {
        "If commands depend on each other and must run sequentially, use a single PowerShell command string. In Windows PowerShell 5.1, do not rely on Bash chaining semantics like `cmd1 && cmd2`; prefer `cmd1; if ($?) { cmd2 }` when the later command depends on earlier success."
    } else {
        "If commands depend on each other and must run sequentially, use a single shell command and chain with `&&` when later commands depend on earlier success."
    };

    let shell = if cfg!(windows) { "powershell" } else { "bash" };

    SHELL_COMMAND_DESCRIPTION
        .replace(
            "${directory}",
            &std::env::current_dir().map_or_else(|_| ".".to_string(), |p| p.display().to_string()),
        )
        .replace("${os}", std::env::consts::OS)
        .replace("${shell}", shell)
        .replace("${chaining}", chaining)
        .replace("${maxBytes}", "64 KB")
}

fn apply_patch_schema() -> JsonSchema {
    JsonSchema::object(
        BTreeMap::from([(
            "patchText".to_string(),
            JsonSchema::string(Some(
                "The full patch text that describes all changes to be made",
            )),
        )]),
        Some(vec!["patchText".to_string()]),
        Some(false),
    )
}

fn plan_schema() -> JsonSchema {
    JsonSchema::object(
        BTreeMap::from([
            (
                "explanation".to_string(),
                JsonSchema::string(Some("Optional explanation for the plan update")),
            ),
            (
                "plan".to_string(),
                JsonSchema::array(
                    JsonSchema::object(
                        BTreeMap::from([
                            (
                                "step".to_string(),
                                JsonSchema::string(Some("Description of the plan step")),
                            ),
                            (
                                "status".to_string(),
                                JsonSchema::string(Some("Status of the step")),
                            ),
                        ]),
                        Some(vec!["step".to_string(), "status".to_string()]),
                        Some(false),
                    ),
                    Some("List of plan items"),
                ),
            ),
        ]),
        Some(vec!["plan".to_string()]),
        Some(false),
    )
}

fn question_schema() -> JsonSchema {
    let option_schema = JsonSchema::object(
        BTreeMap::from([
            (
                "label".to_string(),
                JsonSchema::string(Some("Short option label shown to the user")),
            ),
            (
                "description".to_string(),
                JsonSchema::string(Some("One sentence describing the option tradeoff")),
            ),
        ]),
        Some(vec!["label".to_string(), "description".to_string()]),
        Some(false),
    );
    let question_schema = JsonSchema::object(
        BTreeMap::from([
            (
                "id".to_string(),
                JsonSchema::string(Some("Stable identifier for mapping answers")),
            ),
            (
                "header".to_string(),
                JsonSchema::string(Some("Short header label shown in the UI")),
            ),
            (
                "question".to_string(),
                JsonSchema::string(Some("Single sentence prompt shown to the user")),
            ),
            (
                "isOther".to_string(),
                JsonSchema::boolean(Some("Whether a free-form Other answer is allowed")),
            ),
            (
                "isSecret".to_string(),
                JsonSchema::boolean(Some("Whether free-form text should be treated as secret")),
            ),
            (
                "options".to_string(),
                JsonSchema::array(option_schema, Some("Mutually exclusive answer options")),
            ),
        ]),
        Some(vec![
            "id".to_string(),
            "header".to_string(),
            "question".to_string(),
        ]),
        Some(false),
    );
    JsonSchema::object(
        BTreeMap::from([(
            "questions".to_string(),
            JsonSchema::array(question_schema, Some("Questions to show the user")),
        )]),
        Some(vec!["questions".to_string()]),
        Some(false),
    )
}

fn webfetch_schema() -> JsonSchema {
    JsonSchema::object(
        BTreeMap::from([
            (
                "url".to_string(),
                JsonSchema::string(Some("The URL to fetch content from")),
            ),
            (
                "format".to_string(),
                JsonSchema::string(Some("The format to return (text, markdown, or html)")),
            ),
            (
                "timeout".to_string(),
                JsonSchema::integer(Some("Optional timeout in seconds")),
            ),
        ]),
        Some(vec!["url".to_string()]),
        Some(false),
    )
}

fn app_config_uses_local_web_search(config: &AppConfig) -> bool {
    let provider_catalog = config.provider_catalog_config();
    config.tools.web_search.mode == devo_config::WebSearchMode::Local
        || provider_catalog.providers.values().any(|provider| {
            provider
                .web_search
                .as_ref()
                .is_some_and(|web_search| web_search.mode == devo_config::WebSearchMode::Local)
        })
        || provider_catalog.providers.values().any(|provider| {
            provider.models.values().any(|model| {
                model
                    .web_search
                    .as_ref()
                    .is_some_and(|web_search| web_search.mode == devo_config::WebSearchMode::Local)
            })
        })
}

fn app_config_uses_local_web_fetch(config: &AppConfig) -> bool {
    let provider_catalog = config.provider_catalog_config();
    config.tools.web_fetch.mode == devo_config::WebFetchMode::Local
        || provider_catalog.providers.values().any(|provider| {
            provider
                .web_fetch
                .as_ref()
                .is_some_and(|web_fetch| web_fetch.mode == devo_config::WebFetchMode::Local)
        })
        || provider_catalog.providers.values().any(|provider| {
            provider.models.values().any(|model| {
                model
                    .web_fetch
                    .as_ref()
                    .is_some_and(|web_fetch| web_fetch.mode == devo_config::WebFetchMode::Local)
            })
        })
}

fn websearch_schema() -> JsonSchema {
    JsonSchema::object(
        BTreeMap::from([(
            "query".to_string(),
            JsonSchema::string(Some("The search query")),
        )]),
        Some(vec!["query".to_string()]),
        Some(false),
    )
}

fn lsp_schema() -> JsonSchema {
    JsonSchema::object(
        BTreeMap::from([
            (
                "filePath".to_string(),
                JsonSchema::string(Some("The absolute path to the file")),
            ),
            (
                "line".to_string(),
                JsonSchema::integer(Some("Line number (0-indexed)")),
            ),
            (
                "character".to_string(),
                JsonSchema::integer(Some("Character offset")),
            ),
        ]),
        Some(vec![
            "filePath".to_string(),
            "line".to_string(),
            "character".to_string(),
        ]),
        Some(false),
    )
}

fn exec_command_schema() -> JsonSchema {
    JsonSchema::object(
        BTreeMap::from([
            (
                "cmd".to_string(),
                JsonSchema::string(Some("Shell command to execute.")),
            ),
            (
                "command".to_string(),
                JsonSchema::string(Some("Alias for cmd")),
            ),
            (
                "workdir".to_string(),
                JsonSchema::string(Some("Working directory. Defaults to current directory.")),
            ),
            (
                "shell".to_string(),
                JsonSchema::string(Some(
                    "Shell binary to launch (e.g. 'bash' or 'powershell').",
                )),
            ),
            (
                "login".to_string(),
                JsonSchema::boolean(Some(
                    "Whether to run the shell with login shell semantics. Defaults to true.",
                )),
            ),
            (
                "tty".to_string(),
                JsonSchema::boolean(Some(
                    "Whether to allocate a PTY. Must be true for write_stdin to work.",
                )),
            ),
            (
                "execution_mode".to_string(),
                JsonSchema::string(Some(
                    "attached (default) returns output or a process ID; background returns a task id immediately.",
                )),
            ),
            (
                "yield_time_ms".to_string(),
                JsonSchema::number(Some(
                    "How long to wait (in ms) for output before returning. Default 10000.",
                )),
            ),
            (
                "max_output_tokens".to_string(),
                JsonSchema::number(Some("Maximum number of tokens of output to return.")),
            ),
        ]),
        Some(vec!["cmd".to_string()]),
        Some(false),
    )
}

fn write_stdin_schema() -> JsonSchema {
    JsonSchema::object(
        BTreeMap::from([
            (
                "process_id".to_string(),
                JsonSchema::integer(Some("Process ID of the running exec_command process")),
            ),
            (
                "chars".to_string(),
                JsonSchema::string(Some(
                    "Bytes to write to stdin. Empty string to poll for output.",
                )),
            ),
            (
                "yield_time_ms".to_string(),
                JsonSchema::number(Some(
                    "How long to wait (in ms) for output before returning. Default 250.",
                )),
            ),
            (
                "max_output_tokens".to_string(),
                JsonSchema::number(Some("Maximum number of tokens of output to return.")),
            ),
        ]),
        Some(vec!["process_id".to_string()]),
        Some(false),
    )
}

pub fn build_tool_registry_plan(config: &ToolPlanConfig) -> ToolRegistryPlan {
    config.validate();

    if config.execution_surface == devo_kernel::ExecutionSurface::Rlm {
        return build_rlm_registry_plan();
    }

    let mut plan = ToolRegistryPlan::new();

    if config.use_shell_command {
        plan.push(
            shell_command_tool_spec("shell_command"),
            ToolHandlerKind::ShellCommand,
        );
    } else {
        // Legacy tool name; same handler and schema as shell_command.
        plan.push(
            shell_command_tool_spec("bash"),
            ToolHandlerKind::ShellCommand,
        );
    }

    plan.push(
        ToolSpec {
            name: "apply_patch".to_string(),
            description: APPLY_PATCH_DESCRIPTION.to_string(),
            input_schema: apply_patch_schema(),
            output_mode: ToolOutputMode::Mixed,
            execution_mode: ToolExecutionMode::Mutating,
            capability_tags: vec![ToolCapabilityTag::WriteFiles],
            supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::LiveOnly,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        },
        ToolHandlerKind::ApplyPatch,
    );

    plan.push(
        ToolSpec {
            name: "update_plan".to_string(),
            description: "Updates the task plan.\nProvide an optional explanation and a list of plan items, each with a step and status.\nAt most one step can be in_progress at a time.".to_string(),
            input_schema: plan_schema(),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::Mutating,
            capability_tags: vec![],
            supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        },
        ToolHandlerKind::Plan,
    );

    plan.push(
        ToolSpec {
            name: "request_user_input".to_string(),
            description: "Ask the user one or more questions and wait for the response."
                .to_string(),
            input_schema: question_schema(),
            output_mode: ToolOutputMode::StructuredJson,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: true,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        },
        ToolHandlerKind::Question,
    );

    if config.web_fetch {
        plan.push(
            ToolSpec {
                name: "webfetch".to_string(),
                description: WEBFETCH_DESCRIPTION.to_string(),
                input_schema: webfetch_schema(),
                output_mode: ToolOutputMode::Mixed,
                execution_mode: ToolExecutionMode::ReadOnly,
                capability_tags: vec![ToolCapabilityTag::NetworkAccess],
                supports_parallel: true,
                preparation_feedback: ToolPreparationFeedback::None,
                display_name: None,
                supports_cancellation: None,
                supports_streaming: None,
            },
            ToolHandlerKind::WebFetch,
        );
    }

    if config.web_search {
        plan.push(
            ToolSpec {
                name: "web_search".to_string(),
                description: web_search_prompt(),
                input_schema: websearch_schema(),
                output_mode: ToolOutputMode::Text,
                execution_mode: ToolExecutionMode::ReadOnly,
                capability_tags: vec![ToolCapabilityTag::NetworkAccess],
                supports_parallel: true,
                preparation_feedback: ToolPreparationFeedback::None,
                display_name: None,
                supports_cancellation: None,
                supports_streaming: None,
            },
            ToolHandlerKind::WebSearch,
        );
    }

    plan.push(
        ToolSpec {
            name: "lsp".to_string(),
            description:
                "Get language server protocol information about a file at a specific position."
                    .to_string(),
            input_schema: lsp_schema(),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![ToolCapabilityTag::SearchWorkspace],
            supports_parallel: true,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        },
        ToolHandlerKind::Lsp,
    );

    if config.use_unified_exec {
        plan.push(
            ToolSpec {
                name: "exec_command".to_string(),
                description:
                    "Run a shell command in attached or background mode. Attached mode returns output or a process ID for write_stdin; background mode returns a task id for await_task, list_tasks, and cancel_task."
                        .to_string(),
                input_schema: exec_command_schema(),
                output_mode: ToolOutputMode::Mixed,
                execution_mode: ToolExecutionMode::Mutating,
                capability_tags: vec![ToolCapabilityTag::ExecuteProcess],
                supports_parallel: true,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
            },
            ToolHandlerKind::ExecCommand,
        );
        plan.push(
            ToolSpec {
                name: "write_stdin".to_string(),
                description:
                    "Write bytes to stdin of a running unified exec session, or poll for output without writing. Returns any output produced since the last write_stdin."
                        .to_string(),
                input_schema: write_stdin_schema(),
                output_mode: ToolOutputMode::Mixed,
                execution_mode: ToolExecutionMode::Mutating,
                capability_tags: vec![ToolCapabilityTag::ExecuteProcess],
                supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
            },
            ToolHandlerKind::WriteStdin,
        );
    }

    plan
}

/// Build the internal RLM registry, including the Python kernel and host handlers.
///
/// Registry assembly keeps non-Python handlers available to server-side flows,
/// but marks every model-facing schema except `ipython` Hidden. Hosted web tools
/// are not attached to provider requests, and ToolSearch cannot reveal hidden tools.
pub fn build_rlm_registry_plan() -> ToolRegistryPlan {
    let mut plan = ToolRegistryPlan::new();
    plan.push(
        ToolSpec {
            name: "ipython".to_string(),
            description: "Execute Python code in the session RLM kernel. Namespace persists across cells and turns.".to_string(),
            input_schema: JsonSchema::object(
                BTreeMap::from([(
                    "code".to_string(),
                    JsonSchema::string(Some("Python source to execute")),
                )]),
                Some(vec!["code".to_string()]),
                None,
            ),
            output_mode: ToolOutputMode::Mixed,
            execution_mode: ToolExecutionMode::Mutating,
            capability_tags: vec![],
            supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: Some("ipython".into()),
            supports_cancellation: Some(true),
            supports_streaming: Some(true),
        },
        ToolHandlerKind::Ipython,
    );
    plan.push(
        shell_command_tool_spec("bash"),
        ToolHandlerKind::ShellCommand,
    );
    plan.push(
        ToolSpec {
            name: "update_plan".to_string(),
            description: "Updates the task plan.\nProvide an optional explanation and a list of plan items, each with a step and status.\nAt most one step can be in_progress at a time.".to_string(),
            input_schema: plan_schema(),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::Mutating,
            capability_tags: vec![],
            supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        },
        ToolHandlerKind::Plan,
    );
    plan.push(
        ToolSpec {
            name: "request_user_input".to_string(),
            description: "Ask the user one or more questions and wait for the response."
                .to_string(),
            input_schema: question_schema(),
            output_mode: ToolOutputMode::StructuredJson,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: true,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        },
        ToolHandlerKind::Question,
    );
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn plan_default_starts_empty() {
        let plan = ToolRegistryPlan::new();
        assert!(plan.specs.is_empty());
        assert!(plan.handlers.is_empty());
    }

    #[test]
    fn plan_push_adds_spec_and_handler() {
        let mut plan = ToolRegistryPlan::new();
        plan.push(
            ToolSpec::new("test", "desc", JsonSchema::string(None)),
            ToolHandlerKind::Read,
        );
        assert_eq!(plan.specs.len(), 1);
        assert_eq!(plan.handlers.len(), 1);
        assert_eq!(plan.handlers[0].0, ToolHandlerKind::Read);
        assert_eq!(plan.handlers[0].1, "test");
    }

    #[test]
    fn config_default_has_unified_exec_enabled() {
        let config = ToolPlanConfig::default();
        assert!(config.use_unified_exec);
        assert!(config.use_shell_command);
    }

    #[test]
    fn app_config_selects_matching_registry_execution_surface() {
        let mut app_config = AppConfig::default();
        assert_eq!(
            ToolPlanConfig::from_app_config(&app_config).execution_surface,
            devo_kernel::ExecutionSurface::Rlm
        );

        app_config.tools.execution_surface = devo_config::ToolExecutionSurface::Discrete;
        assert_eq!(
            ToolPlanConfig::from_app_config(&app_config).execution_surface,
            devo_kernel::ExecutionSurface::Discrete
        );
    }

    #[test]
    fn config_validate_does_not_panic() {
        let config = ToolPlanConfig::default();
        config.validate(); // should not panic
    }

    #[test]
    fn schema_exec_command_requires_cmd() {
        let schema = exec_command_schema();
        let required = schema.required.as_ref().unwrap();
        assert!(required.contains(&"cmd".to_string()));
        assert!(
            schema
                .properties
                .as_ref()
                .is_some_and(|properties| properties.contains_key("execution_mode"))
        );
    }

    #[test]
    fn schema_write_stdin_requires_process_id() {
        let schema = write_stdin_schema();
        let required = schema.required.as_ref().unwrap();
        assert_eq!(required, &vec!["process_id".to_string()]);
    }

    #[test]
    fn default_registry_does_not_expose_an_always_failing_tool() {
        let plan = build_tool_registry_plan(&ToolPlanConfig::default());

        assert!(plan.specs.iter().all(|spec| spec.name != "invalid"));
        assert!(plan.handlers.iter().all(|(_, name)| name != "invalid"));
    }

    #[test]
    fn shell_command_schema_has_command_and_cmd() {
        let schema = shell_command_schema();
        let props = schema.properties.as_ref().unwrap();
        assert!(props.contains_key("command"));
        assert!(props.contains_key("cmd"));
        assert!(props.contains_key("timeout_ms"));
        assert!(props.contains_key("tty"));
    }

    #[test]
    fn plan_builder_without_unified_exec() {
        let plan = build_tool_registry_plan(&ToolPlanConfig {
            use_unified_exec: false,
            ..ToolPlanConfig::default()
        });
        let handler_names: Vec<&str> = plan.handlers.iter().map(|(_, n)| n.as_str()).collect();
        assert!(!handler_names.contains(&"exec_command"));
        assert!(!handler_names.contains(&"write_stdin"));
    }

    #[test]
    fn plan_builder_registers_shell_command_not_bash_by_default() {
        let plan = build_tool_registry_plan(&ToolPlanConfig::default());
        let spec_names: Vec<&str> = plan.specs.iter().map(|spec| spec.name.as_str()).collect();

        assert!(spec_names.contains(&"shell_command"));
        assert!(!spec_names.contains(&"bash"));
        assert!(
            plan.handlers
                .iter()
                .any(|(kind, name)| *kind == ToolHandlerKind::ShellCommand
                    && name == "shell_command")
        );
    }

    #[test]
    fn plan_builder_registers_web_search_only_when_local_enabled() {
        let default_plan = build_tool_registry_plan(&ToolPlanConfig::default());
        let default_spec_names: Vec<&str> = default_plan
            .specs
            .iter()
            .map(|spec| spec.name.as_str())
            .collect();
        assert!(!default_spec_names.contains(&"web_search"));

        let local_plan = build_tool_registry_plan(&ToolPlanConfig {
            web_search: true,
            ..ToolPlanConfig::default()
        });
        let local_spec_names: Vec<&str> = local_plan
            .specs
            .iter()
            .map(|spec| spec.name.as_str())
            .collect();
        assert!(local_spec_names.contains(&"web_search"));
        assert!(!local_spec_names.contains(&"websearch"));
        let web_search_spec = local_plan
            .specs
            .iter()
            .find(|spec| spec.name == "web_search")
            .expect("web_search spec");
        assert!(web_search_spec.description.contains("Sources:"));
    }

    #[test]
    fn plan_builder_registers_webfetch_only_when_local_enabled() {
        let local_plan = build_tool_registry_plan(&ToolPlanConfig::default());
        let local_spec_names: Vec<&str> = local_plan
            .specs
            .iter()
            .map(|spec| spec.name.as_str())
            .collect();
        assert!(local_spec_names.contains(&"webfetch"));

        let provider_plan = build_tool_registry_plan(&ToolPlanConfig {
            web_fetch: false,
            ..ToolPlanConfig::default()
        });
        let provider_spec_names: Vec<&str> = provider_plan
            .specs
            .iter()
            .map(|spec| spec.name.as_str())
            .collect();
        assert!(!provider_spec_names.contains(&"webfetch"));
    }

    /// Trace: kernel-tools-only design (user direction, R27)
    /// Verifies: the stale host-side file tools (read/write/edit/find/grep)
    /// are gone from every registry plan — file work goes through the shell
    /// tool (and the python kernel's rlm front door on the kernel surface).
    #[test]
    fn plan_builder_omits_stale_host_file_tools() {
        for config in [
            ToolPlanConfig::default(),
            ToolPlanConfig {
                execution_surface: devo_kernel::ExecutionSurface::Rlm,
                ..ToolPlanConfig::default()
            },
        ] {
            let plan = build_tool_registry_plan(&config);
            let spec_names: Vec<&str> = plan.specs.iter().map(|spec| spec.name.as_str()).collect();
            for stale in ["read", "write", "edit", "find", "grep"] {
                assert!(
                    !spec_names.contains(&stale),
                    "stale file tool {stale} still registered: {spec_names:?}"
                );
            }
        }
    }

    /// Trace: prompt-payload audit (R27)
    /// Verifies: the shell description never steers the model toward the
    /// removed file tools — it used to instruct "Use Read (NOT cat/head/tail)"
    /// in kernel sessions where no such tool exists, sending the model on
    /// ToolSearch hunts for tools that are not there.
    #[test]
    fn shell_description_references_no_removed_file_tools() {
        for config in [
            ToolPlanConfig::default(),
            ToolPlanConfig {
                execution_surface: devo_kernel::ExecutionSurface::Rlm,
                ..ToolPlanConfig::default()
            },
        ] {
            let plan = build_tool_registry_plan(&config);
            let shell = plan
                .specs
                .iter()
                .find(|spec| spec.name == "shell_command" || spec.name == "bash")
                .expect("shell tool spec");
            for stale in [
                "Use Read",
                "Use Edit",
                "Use Write",
                "NOT cat/head/tail",
                "NOT sed/awk",
            ] {
                assert!(
                    !shell.description.contains(stale),
                    "shell description still references removed tool guidance: {stale}"
                );
            }
            assert!(
                shell
                    .description
                    .contains("There are no separate file tools")
            );
        }
    }

    /// Trace: L2-DES-MCP-002
    /// Verifies: native code_search is no longer registered; retrieval is MCP-only.
    #[test]
    fn plan_builder_omits_native_code_search() {
        let plan = build_tool_registry_plan(&ToolPlanConfig::default());
        let spec_names: Vec<&str> = plan.specs.iter().map(|spec| spec.name.as_str()).collect();
        let handler_names: Vec<&str> = plan
            .handlers
            .iter()
            .map(|(_, name)| name.as_str())
            .collect();

        assert!(!spec_names.contains(&"code_search"));
        assert!(!handler_names.contains(&"code_search"));
    }

    /// Trace: L2-DES-RLM-001
    /// Verifies: the internal RLM plan retains Python, shell, plan, and question handlers.
    #[test]
    fn rlm_runtime_plan_includes_ipython_and_bash_handlers() {
        let plan = build_rlm_registry_plan();
        let names: Vec<&str> = plan.specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["ipython", "bash", "update_plan", "request_user_input"]
        );
        assert!(
            plan.handlers
                .iter()
                .any(|(kind, name)| *kind == ToolHandlerKind::Ipython && name == "ipython")
        );
        assert!(
            plan.handlers
                .iter()
                .any(|(kind, name)| *kind == ToolHandlerKind::ShellCommand && name == "bash")
        );
        assert!(
            plan.handlers
                .iter()
                .any(|(kind, name)| *kind == ToolHandlerKind::Question
                    && name == "request_user_input")
        );
    }

    /// Trace: L2-DES-RLM-001
    /// Verifies: the discrete internal plan retains shell and patch handlers; the
    /// registry exposure policy hides their schemas from model requests.
    #[test]
    fn discrete_plan_keeps_shell_and_patch_tools() {
        let config = ToolPlanConfig {
            execution_surface: devo_kernel::ExecutionSurface::Discrete,
            ..ToolPlanConfig::default()
        };
        let plan = build_tool_registry_plan(&config);
        let names: Vec<&str> = plan.specs.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"shell_command"),
            "discrete registry must keep the shell tool, got {names:?}"
        );
        assert!(
            names.contains(&"apply_patch"),
            "discrete registry must keep apply_patch, got {names:?}"
        );
        assert!(
            !names.iter().all(|n| *n == "ipython"),
            "discrete must not collapse to ipython-only"
        );
    }

    /// Trace: L2-DES-RLM-001
    /// Verifies: Rlm execution_surface selects the RLM root plan.
    #[test]
    fn rlm_surface_selects_rlm_root_plan() {
        let config = ToolPlanConfig {
            execution_surface: devo_kernel::ExecutionSurface::Rlm,
            ..ToolPlanConfig::default()
        };
        let plan = build_tool_registry_plan(&config);
        let names: Vec<&str> = plan.specs.iter().map(|s| s.name.as_str()).collect();
        // Plan updates and user questions remain available through RLM host
        // requests; these handlers are not model-facing schemas.
        assert_eq!(
            names,
            vec!["ipython", "bash", "update_plan", "request_user_input"]
        );
    }
}

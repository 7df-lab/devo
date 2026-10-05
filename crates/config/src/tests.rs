use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use devo_protocol::PermissionPreset;
use pretty_assertions::assert_eq;

use super::AppConfig;
use super::AppConfigLoader;
use super::AppConfigStore;
use super::CommandHookConfig;
use super::ExperimentalConfig;
use super::FileSystemAppConfigLoader;
use super::HookCommandConfig;
use super::HookEvent;
use super::HookMatcherConfig;
use super::HookShell;
use super::HooksConfig;
use super::LogRotation;
use super::McpOutputLimits;
use super::McpRootsPolicy;
use super::McpServerId;
use super::McpServerRecord;
use super::McpStartupPolicy;
use super::McpTransportConfig;
use super::McpTrustPolicy;
use super::ModelOverrideConfig;
use super::PatternMode;
use super::PermissionConfig;
use super::PermissionRule;
use super::ProjectConfig;
use super::PromptPolicy;
use super::ProviderHttpConfig;
use super::RuleAction;
use super::SummaryModelSelection;
use super::ToolFilter;
use super::UpdatesConfig;
use devo_protocol::ProviderInfo;
use devo_protocol::ProviderModelInfo;
use devo_protocol::ProviderWireApi;
use devo_protocol::ReasoningCapability;
use devo_protocol::ReasoningEffort;
use devo_protocol::ReasoningLevelChoice;
use devo_protocol::TruncationPolicyConfig;

fn unique_temp_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("devo-{name}-{nanos}"));
    std::fs::create_dir_all(&path).expect("create temp dir");
    path
}

struct ConfigFixture {
    root: PathBuf,
    home: PathBuf,
    workspace: Option<PathBuf>,
}

impl ConfigFixture {
    fn new(name: &str) -> Self {
        let root = unique_temp_dir(name);
        let home = root.join("home").join(".devo");
        std::fs::create_dir_all(&home).expect("home config dir");
        Self {
            root,
            home,
            workspace: None,
        }
    }

    fn with_workspace(mut self) -> Self {
        let workspace = self.root.join("workspace");
        std::fs::create_dir_all(workspace.join(".devo")).expect("workspace config dir");
        self.workspace = Some(workspace);
        self
    }

    fn write_home(&self, content: &str) {
        std::fs::write(self.home.join("config.toml"), content).expect("write user config");
    }

    fn write_workspace(&self, content: &str) {
        let workspace = self.workspace.as_ref().expect("workspace fixture");
        std::fs::write(workspace.join(".devo").join("config.toml"), content)
            .expect("write project config");
    }

    fn loader(&self) -> FileSystemAppConfigLoader {
        FileSystemAppConfigLoader::new(self.home.clone())
    }

    fn load(&self) -> AppConfig {
        self.loader()
            .load(self.workspace.as_deref())
            .expect("load config")
    }

    fn load_err(&self) -> super::AppConfigError {
        self.loader()
            .load(self.workspace.as_deref())
            .expect_err("load config")
    }
}

impl Drop for ConfigFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn mcp_record(id: &str, transport: McpTransportConfig) -> McpServerRecord {
    McpServerRecord {
        id: McpServerId(id.to_string()),
        display_name: id.to_string(),
        transport,
        startup_policy: McpStartupPolicy::Lazy,
        enabled: true,
        trust_policy: McpTrustPolicy::User,
        allowed_capabilities: Vec::new(),
        roots_policy: McpRootsPolicy::None,
        output_limits: McpOutputLimits::default(),
        auth_ref: None,
    }
}

#[test]
fn loader_merges_user_project_and_cli_layers() {
    let fixture = ConfigFixture::new("config-merge").with_workspace();
    fixture.write_home(
        "default_model = 'ignored'\n[anthropic]\nmodel = 'also-ignored'\n[context]\npreserve_recent_turns = 5\n[logging]\nlevel = 'debug'\n[logging.file]\nmax_files = 30\n",
    );
    fixture.write_workspace(
        "enable_auxiliary_model = true\nproject_root_markers = ['.git', 'Cargo.toml']\n[context]\nauto_compact_percent = 80\n[logging]\njson = true\n[logging.file]\ndirectory = 'diagnostics'\nfilename_prefix = 'agent'\n[skills]\nenabled = true\nworkspace_roots = ['project-skills']\nwatch_for_changes = false\n",
    );
    let cli_overrides: toml::Value = r#"
summary_model = "UseAxiliaryModel"
project_root_markers = [".workspace"]

[server]
listen = ["stdio://"]

[logging]
level = "trace"

[logging.file]
directory = "cli-logs"
rotation = "Hourly"
max_files = 2

[skills]
enabled = false
user_roots = ["custom-user-skills"]

[updates]
enabled = false
check_interval_hours = 48
"#
    .parse()
    .expect("parse cli overrides");

    let config = FileSystemAppConfigLoader::new(fixture.home.clone())
        .with_cli_overrides(cli_overrides)
        .load(fixture.workspace.as_deref())
        .expect("load config");

    assert_eq!(
        config.summary_model,
        SummaryModelSelection::UseAxiliaryModel
    );
    assert_eq!(config.server.listen, vec!["stdio://".to_string()]);
    assert_eq!(config.logging.level, "trace");
    assert_eq!(
        config.logging.file,
        super::LoggingFileConfig {
            directory: Some(PathBuf::from("cli-logs")),
            filename_prefix: "agent".into(),
            rotation: LogRotation::Hourly,
            max_files: 2,
        }
    );
    assert_eq!(config.skills.enabled, false);
    assert_eq!(
        config.skills.user_roots,
        vec![PathBuf::from("custom-user-skills")]
    );
    assert_eq!(
        config.skills.workspace_roots,
        vec![PathBuf::from("project-skills")]
    );
    assert_eq!(config.updates.check_interval_hours, 48);
    assert_eq!(config.project_root_markers, vec![".workspace".to_string()]);
}

#[test]
fn loader_defaults_permission_config_when_section_is_absent() {
    let fixture = ConfigFixture::new("permission-default");
    assert_eq!(fixture.load().permission, PermissionConfig::default());
}

#[test]
fn loader_reads_permission_rules_and_auto_default_mode() {
    let fixture = ConfigFixture::new("permission-rules");
    fixture.write_home(
        r#"
[permission]
default_mode = "auto"

[[permission.rules]]
action = "allow"
tool = "bash"
pattern = "git *"

[[permission.rules]]
action = "deny"
tool = "edit"
pattern = "**/.env"

[[permission.rules]]
action = "ask"
tool = "web_fetch"
pattern = "example.com"
pattern_mode = "domain"

[[permission.rules]]
tool = "read"
"#,
    );
    assert_eq!(
        fixture.load().permission,
        PermissionConfig {
            rules: vec![
                PermissionRule {
                    action: RuleAction::Allow,
                    tool: ToolFilter::Bash,
                    pattern: Some("git *".to_string()),
                    pattern_mode: PatternMode::Glob,
                },
                PermissionRule {
                    action: RuleAction::Deny,
                    tool: ToolFilter::Edit,
                    pattern: Some("**/.env".to_string()),
                    pattern_mode: PatternMode::Glob,
                },
                PermissionRule {
                    action: RuleAction::Ask,
                    tool: ToolFilter::WebFetch,
                    pattern: Some("example.com".to_string()),
                    pattern_mode: PatternMode::Domain,
                },
                PermissionRule {
                    action: RuleAction::Deny,
                    tool: ToolFilter::Read,
                    pattern: None,
                    pattern_mode: PatternMode::Glob,
                },
            ],
            prompt_policy: PromptPolicy::Auto,
            sandbox_profile: None,
            warn_unfenced_kernel: true,
        }
    );
}

#[test]
fn loader_reads_permission_sandbox_profile() {
    let fixture = ConfigFixture::new("permission-sandbox-profile");
    fixture.write_home("[permission]\nsandbox_profile = \"off\"\n");
    assert_eq!(
        fixture.load().permission.sandbox_profile,
        Some("off".to_string())
    );
}

#[test]
fn default_app_config_serializes_permission_default_mode() {
    let serialized = toml::Value::try_from(AppConfig::default()).expect("serialize config");

    assert_eq!(
        serialized
            .get("permission")
            .and_then(toml::Value::as_table)
            .and_then(|permission| permission.get("default_mode"))
            .and_then(toml::Value::as_str),
        Some("ask")
    );
}

#[test]
fn loader_rejects_invalid_permission_rule_action() {
    let fixture = ConfigFixture::new("permission-invalid-action");
    fixture.write_home("[[permission.rules]]\naction = 'approve'\n");
    assert!(matches!(
        fixture.load_err(),
        super::AppConfigError::Parse { .. }
    ));
}

#[test]
fn loader_permission_overlay_precedence_table() {
    struct Case {
        name: &'static str,
        home: Option<&'static str>,
        workspace: Option<&'static str>,
        cli: Option<&'static str>,
        expected: PermissionConfig,
    }
    let cases = [
        Case {
            name: "workspace_omits_user_permission",
            home: Some(
                "[permission]\ndefault_mode = 'deny'\n[[permission.rules]]\naction = 'allow'\ntool = 'web_search'\n",
            ),
            workspace: Some("[logging]\nlevel = 'debug'\n"),
            cli: None,
            expected: PermissionConfig {
                rules: vec![PermissionRule {
                    action: RuleAction::Allow,
                    tool: ToolFilter::WebSearch,
                    pattern: None,
                    pattern_mode: PatternMode::Glob,
                }],
                prompt_policy: PromptPolicy::Deny,
                sandbox_profile: None,
                warn_unfenced_kernel: true,
            },
        },
        Case {
            name: "workspace_replaces_user_permission",
            home: Some(
                "[permission]\ndefault_mode = 'auto'\n[[permission.rules]]\naction = 'allow'\ntool = 'bash'\n",
            ),
            workspace: Some(
                "[permission]\ndefault_mode = 'deny'\n[[permission.rules]]\naction = 'ask'\ntool = 'mcp'\n",
            ),
            cli: None,
            expected: PermissionConfig {
                rules: vec![PermissionRule {
                    action: RuleAction::Ask,
                    tool: ToolFilter::Mcp,
                    pattern: None,
                    pattern_mode: PatternMode::Glob,
                }],
                prompt_policy: PromptPolicy::Deny,
                sandbox_profile: None,
                warn_unfenced_kernel: true,
            },
        },
        Case {
            name: "cli_omits_workspace_permission",
            home: None,
            workspace: Some(
                "[permission]\ndefault_mode = 'auto'\n[[permission.rules]]\naction = 'allow'\ntool = 'bash'\npattern = 'git *'\n",
            ),
            cli: Some("[logging]\nlevel = 'trace'\n"),
            expected: PermissionConfig {
                rules: vec![PermissionRule {
                    action: RuleAction::Allow,
                    tool: ToolFilter::Bash,
                    pattern: Some("git *".to_string()),
                    pattern_mode: PatternMode::Glob,
                }],
                prompt_policy: PromptPolicy::Auto,
                sandbox_profile: None,
                warn_unfenced_kernel: true,
            },
        },
        Case {
            name: "cli_replaces_workspace_permission",
            home: None,
            workspace: Some(
                "[permission]\ndefault_mode = 'auto'\n[[permission.rules]]\naction = 'allow'\ntool = 'bash'\npattern = 'git *'\n",
            ),
            cli: Some(
                r#"[permission]
default_mode = "deny"

[[permission.rules]]
action = "ask"
tool = "mcp"
pattern = "deploy"
"#,
            ),
            expected: PermissionConfig {
                rules: vec![PermissionRule {
                    action: RuleAction::Ask,
                    tool: ToolFilter::Mcp,
                    pattern: Some("deploy".to_string()),
                    pattern_mode: PatternMode::Glob,
                }],
                prompt_policy: PromptPolicy::Deny,
                sandbox_profile: None,
                warn_unfenced_kernel: true,
            },
        },
    ];
    for case in cases {
        let fixture = ConfigFixture::new(case.name).with_workspace();
        if let Some(home) = case.home {
            fixture.write_home(home);
        }
        if let Some(workspace) = case.workspace {
            fixture.write_workspace(workspace);
        }
        let mut loader = FileSystemAppConfigLoader::new(fixture.home.clone());
        if let Some(cli) = case.cli {
            loader = loader.with_cli_overrides(cli.parse().expect("cli"));
        }
        assert_eq!(
            loader
                .load(fixture.workspace.as_deref())
                .expect("load")
                .permission,
            case.expected,
            "{}",
            case.name
        );
    }
}

#[test]
fn default_app_config_server_and_bundled_mcp() {
    let default = AppConfig::default();
    assert_eq!(
        default.server.auth,
        super::ServerAuthConfig {
            enabled: false,
            method_id: "agent-login".to_string(),
            name: "Agent login".to_string(),
            description: None,
            logout: true,
        }
    );
    assert!(
        !default
            .mcp_runtime
            .servers
            .iter()
            .find(|record| record.id.0 == super::BUNDLED_CODE_SEARCH_MCP_SERVER_ID)
            .expect("bundled code_search server")
            .enabled
    );
    assert_eq!(
        default.updates,
        UpdatesConfig {
            enabled: true,
            check_on_startup: true,
            check_interval_hours: 24,
        }
    );
}

#[test]
fn loader_reads_server_auth_config() {
    let fixture = ConfigFixture::new("config-server-auth");
    fixture.write_home(
        r#"
[server.auth]
enabled = true
method_id = "company-login"
name = "Company login"
description = "Sign in with company credentials"
logout = false
"#,
    );
    assert_eq!(
        fixture.load().server.auth,
        super::ServerAuthConfig {
            enabled: true,
            method_id: "company-login".to_string(),
            name: "Company login".to_string(),
            description: Some("Sign in with company credentials".to_string()),
            logout: false,
        }
    );
}

#[test]
fn loader_rejects_invalid_server_auth_fields_when_enabled() {
    let cases = [
        (
            "config-server-auth-empty-method",
            "[server.auth]\nenabled = true\nmethod_id = '   '\n",
            "server.auth.method_id must not be empty when server auth is enabled",
        ),
        (
            "config-server-auth-empty-name",
            "[server.auth]\nenabled = true\nname = '   '\n",
            "server.auth.name must not be empty when server auth is enabled",
        ),
    ];
    for (name, config, message) in cases {
        let fixture = ConfigFixture::new(name);
        fixture.write_home(config);
        match fixture.load_err() {
            super::AppConfigError::Validation { message: actual } => assert_eq!(actual, message),
            other => panic!("expected server auth validation error, got {other:?}"),
        }
    }
}

#[test]
fn loader_bundled_code_search_mcp_table() {
    let cases = [
        (
            "config-experimental-legacy",
            "[experimental]\ncode-search = true\ncode_search = false\n",
        ),
        ("config-bundled-mcp-ensure", "[mcp]\nauto_start = true\n"),
    ];
    for (name, config) in cases {
        let fixture = ConfigFixture::new(name);
        fixture.write_home(config);
        let loaded = fixture.load();
        if name == "config-experimental-legacy" {
            assert_eq!(loaded.experimental, ExperimentalConfig::default());
        } else {
            assert_eq!(loaded.mcp_runtime.servers.len(), 1);
            assert_eq!(
                loaded.mcp_runtime.servers[0].id.0,
                super::BUNDLED_CODE_SEARCH_MCP_SERVER_ID
            );
        }
        assert!(
            !loaded
                .mcp_runtime
                .servers
                .iter()
                .find(|record| record.id.0 == super::BUNDLED_CODE_SEARCH_MCP_SERVER_ID)
                .expect("bundled code_search server")
                .enabled
        );
    }
}

/// Trace: L2-DES-MCP-002
/// Verifies: enabling bundled code_search materializes it into user config.toml.
#[test]
fn set_mcp_server_enabled_materializes_bundled_code_search() {
    let fixture = ConfigFixture::new("config-bundled-mcp-enable");
    fixture.write_home("[mcp]\nauto_start = true\n");
    let config_file = fixture.home.join("config.toml");
    let mut store =
        AppConfigStore::load(fixture.home.clone(), /*workspace_root*/ None).expect("load store");
    assert!(
        !std::fs::read_to_string(&config_file)
            .expect("read user config")
            .contains("code_search")
    );

    store
        .set_mcp_server_enabled(
            super::BUNDLED_CODE_SEARCH_MCP_SERVER_ID,
            /*enabled*/ true,
        )
        .expect("enable bundled code_search");

    let server = store
        .mcp_servers()
        .iter()
        .find(|record| record.id.0 == super::BUNDLED_CODE_SEARCH_MCP_SERVER_ID)
        .expect("bundled code_search server");
    assert!(server.enabled);

    let user_config = std::fs::read_to_string(&config_file).expect("read user config");
    assert!(user_config.contains("code_search"));
    assert!(user_config.contains("devo-code-search-mcp"));
}

#[test]
fn loader_reads_hook_command_config() {
    let fixture = ConfigFixture::new("config-hooks");
    fixture.write_home(
        r#"
[[hooks.PreToolUse]]
matcher = "exec_command"

[[hooks.PreToolUse.hooks]]
type = "command"
command = "hooks/pre_tool.sh"
shell = "powershell"
timeout = 5
statusMessage = "Checking tool use"
"#,
    );
    assert_eq!(
        fixture.load().hooks,
        HooksConfig(BTreeMap::from([(
            HookEvent::PreToolUse,
            vec![HookMatcherConfig {
                matcher: Some("exec_command".to_string()),
                hooks: vec![HookCommandConfig::Command(CommandHookConfig {
                    command: "hooks/pre_tool.sh".to_string(),
                    shell: Some(HookShell::PowerShell),
                    condition: None,
                    timeout: Some(5),
                    status_message: Some("Checking tool use".to_string()),
                    once: None,
                    async_hook: None,
                    async_rewake: None,
                })],
            }],
        )]))
    );
}

/// Trace: L2-DES-APP-005
/// Verifies: provider HTTP proxy settings and provider header fields follow user/workspace merge precedence.
#[test]
fn loader_merges_provider_sections_with_provider_overlay_rules() {
    let fixture = ConfigFixture::new("config-provider-merge").with_workspace();
    fixture.write_home(
        r#"
[provider_http]
proxy_url = "http://user-proxy.example:8080"

[defaults]
model_binding = "main"

[providers.main]
name = "User Provider"
base_url = "https://user.example/v1"
credential = "user_api_key"
headers = '{"X-User":"yes"}'
wire_apis = ["openai_responses"]

[model_bindings.main]
model_slug = "user-model"
provider = "main"
request_model = "user/model"
invocation_method = "openai_responses"
"#,
    );
    fixture.write_workspace(
        r#"
[provider_http]
proxy_url = "http://workspace-proxy.example:8080"

[providers.main]
name = "Project Provider"

[model_bindings.main]
model_slug = "project-model"
provider = "main"
request_model = "project/model"
invocation_method = "openai_responses"
"#,
    );
    let config = fixture.load();
    assert_eq!(
        config.provider_http,
        ProviderHttpConfig {
            proxy_url: Some("http://workspace-proxy.example:8080".to_string()),
            no_proxy: None,
        }
    );
    assert_eq!(
        config.provider_http.proxy_url.as_deref(),
        Some("http://workspace-proxy.example:8080")
    );
    assert_eq!(
        config.provider_catalog.providers["main"].name.as_deref(),
        Some("Project Provider")
    );
    assert_eq!(
        config.provider_catalog.providers["main"]
            .base_url
            .as_deref(),
        Some("https://user.example/v1")
    );
    assert_eq!(
        config.provider_catalog.providers["main"].headers,
        Some(BTreeMap::from([("X-User".to_string(), "yes".to_string())]))
    );
    assert_eq!(
        config.provider_catalog.providers["main"].models["user/model"].wire_api,
        Some(ProviderWireApi::OpenAIResponses)
    );
    assert_eq!(
        config.provider_catalog.providers["main"].models["project/model"].wire_api,
        Some(ProviderWireApi::OpenAIResponses)
    );
}

/// Trace: L2-DES-APP-005
/// Verifies: omitted defaulted provider fields in a higher-priority partial overlay do not overwrite lower-priority values.
#[test]
fn loader_provider_overlay_preserves_absent_defaulted_provider_fields() {
    let fixture = ConfigFixture::new("config-provider-defaulted-overlay").with_workspace();
    fixture.write_home(
        r#"
[defaults]
model_binding = "main"

[providers.main]
name = "User Provider"
base_url = "https://user.example/v1"
credential = "user_api_key"
headers = '{"X-User":"yes"}'
wire_apis = ["openai_responses"]
enabled = false

[model_bindings.main]
model_slug = "user-model"
provider = "main"
request_model = "user/model"
invocation_method = "openai_responses"
enabled = false
"#,
    );
    fixture.write_workspace(
        r#"
[providers.main]
name = "Project Provider"

[model_bindings.main]
model_slug = "project-model"
provider = "main"
request_model = "project/model"
"#,
    );
    let config = fixture.load();
    assert_eq!(
        config.provider_catalog.providers["main"].enabled,
        Some(false)
    );
    assert_eq!(
        config.provider_catalog.providers["main"].models["user/model"].enabled,
        Some(false)
    );
    assert_eq!(
        config.provider_catalog.providers["main"].models["project/model"].enabled,
        None
    );
}

#[test]
fn loader_applies_workspace_model_overrides_onto_provider_catalog() {
    let fixture = ConfigFixture::new("config-workspace-model-override-catalog").with_workspace();
    fixture.write_home(
        r#"
[defaults]
model_binding = "test-openai"

[providers.openai]
enabled = true
name = "OpenAI"
wire_apis = ["openai_chat_completions"]

[model_bindings.test-openai]
enabled = true
model_slug = "test-model"
provider = "openai"
model_name = "test-model"
invocation_method = "openai_chat_completions"

[model_bindings.alt-openai]
enabled = true
model_slug = "alt-model"
provider = "openai"
model_name = "alt-model"
invocation_method = "openai_chat_completions"
"#,
    );
    fixture.write_workspace(
        r#"
[model.test-model]
display_name = "Test Model"
reasoning_capability = { levels = ["low", "medium", "high"] }
default_reasoning_effort = "medium"
base_instructions = "Test model instructions"

[model.alt-model]
display_name = "Alt Model"
base_instructions = "Alt model instructions"
"#,
    );
    let loader = FileSystemAppConfigLoader::new(fixture.home.clone());
    // Simulate server bootstrap (migrates home bindings) then a later
    // workspace-scoped session load.
    let _ = loader
        .load(/*workspace_root*/ None)
        .expect("bootstrap load");
    let config = loader
        .load(fixture.workspace.as_deref())
        .expect("session load");
    let test_model = &config.provider_catalog.providers["openai"].models["test-model"];
    assert_eq!(test_model.name.as_deref(), Some("Test Model"));
    assert_eq!(
        test_model.reasoning_capability,
        Some(ReasoningCapability::Levels(vec![
            ReasoningLevelChoice::Effort(ReasoningEffort::Low),
            ReasoningLevelChoice::Effort(ReasoningEffort::Medium),
            ReasoningLevelChoice::Effort(ReasoningEffort::High),
        ]))
    );
    assert_eq!(
        test_model.default_reasoning_effort,
        Some(ReasoningEffort::Medium)
    );
    let alt_model = &config.provider_catalog.providers["openai"].models["alt-model"];
    assert_eq!(alt_model.name.as_deref(), Some("Alt Model"));
    assert_eq!(alt_model.reasoning_capability, None);
}

#[test]
fn loader_merges_model_overrides_field_by_field_across_layers() {
    let fixture = ConfigFixture::new("config-model-overrides-overlay").with_workspace();
    fixture.write_home(
        r#"
[model.grok-4]
display_name = "User Grok"
description = "User description"
context_window = 128000
temperature = 0.4
provider = "openai_chat_completions"
default_reasoning_effort = "medium"
truncation_policy = { mode = "tokens", limit = 8000 }
"#,
    );
    fixture.write_workspace(
        r#"
[model.grok-4]
description = "Workspace description"
context_window = 192000
top_p = 0.9
"#,
    );
    let cli_overrides: toml::Value = r#"
[model.grok-4]
display_name = "CLI Grok"
temperature = 0.2

[model.grok-4-mini]
display_name = "Grok 4 Mini"
max_tokens = 4096
"#
    .parse()
    .expect("parse cli overrides");
    let config = FileSystemAppConfigLoader::new(fixture.home.clone())
        .with_cli_overrides(cli_overrides)
        .load(fixture.workspace.as_deref())
        .expect("load config");

    assert_eq!(
        config.provider.model_overrides,
        BTreeMap::from([
            (
                "grok-4".to_string(),
                ModelOverrideConfig {
                    display_name: Some("CLI Grok".to_string()),
                    description: Some("Workspace description".to_string()),
                    context_window: Some(192_000),
                    temperature: Some(0.2),
                    top_p: Some(0.9),
                    provider: Some(ProviderWireApi::OpenAIChatCompletions),
                    default_reasoning_effort: Some(ReasoningEffort::Medium),
                    truncation_policy: Some(TruncationPolicyConfig::tokens(8_000)),
                    ..ModelOverrideConfig::default()
                },
            ),
            (
                "grok-4-mini".to_string(),
                ModelOverrideConfig {
                    display_name: Some("Grok 4 Mini".to_string()),
                    max_tokens: Some(4_096),
                    ..ModelOverrideConfig::default()
                },
            ),
        ])
    );
}

/// Trace: L2-DES-APP-005
/// Verifies: CLI provider overrides participate in the same provider merge precedence as other CLI config.
#[test]
fn loader_applies_cli_provider_overrides_to_provider_section() {
    let fixture = ConfigFixture::new("config-provider-cli-overlay");
    fixture.write_home(
        r#"
[defaults]
model_binding = "main"

[providers.main]
name = "User Provider"
base_url = "https://user.example/v1"
credential = "user_api_key"
wire_apis = ["openai_responses"]

[model_bindings.main]
model_slug = "user-model"
provider = "main"
request_model = "user/model"
invocation_method = "openai_responses"
"#,
    );
    let cli_overrides: toml::Value = r#"
[providers.main]
name = "CLI Provider"
enabled = false

[model_bindings.main]
model_slug = "cli-model"
provider = "main"
request_model = "cli/model"
invocation_method = "openai_responses"
enabled = false
"#
    .parse()
    .expect("parse cli overrides");
    let config = FileSystemAppConfigLoader::new(fixture.home.clone())
        .with_cli_overrides(cli_overrides)
        .load(None)
        .expect("load config");

    assert_eq!(
        config.provider_catalog.providers["main"].name.as_deref(),
        Some("CLI Provider")
    );
    assert_eq!(
        config.provider_catalog.providers["main"].enabled,
        Some(false)
    );
    assert_eq!(
        config.provider_catalog.providers["main"].models["user/model"].enabled,
        None
    );
    assert_eq!(
        config.provider_catalog.providers["main"].models["cli/model"].enabled,
        Some(false)
    );
}

/// Trace: L2-DES-APP-005
/// Verifies: provider upsert persists custom provider header JSON in user config and projections.
#[test]
fn provider_upsert_writes_user_config_when_workspace_is_active() {
    let fixture = ConfigFixture::new("provider-upsert-user").with_workspace();
    let mut store = AppConfigStore::load(fixture.home.clone(), fixture.workspace.as_deref())
        .expect("load store");
    let written_provider = store
        .upsert_provider_connection(
            ProviderInfo {
                id: "openrouter".to_string(),
                name: "openrouter".to_string(),
                description: None,
                base_url: Some("https://openrouter.ai/api/v1".to_string()),
                credential: None,
                headers: BTreeMap::from([("X-Devo".to_string(), "yes".to_string())]),
                options: None,
                request: None,
                compat: None,
                model_overrides: BTreeMap::new(),
                wire_apis: vec![ProviderWireApi::OpenAIChatCompletions],
                models: BTreeMap::from([(
                    "qwen/qwen3".to_string(),
                    ProviderModelInfo {
                        name: Some("Qwen".to_string()),
                        wire_api: Some(ProviderWireApi::OpenAIChatCompletions),
                        default_reasoning_effort: Some(ReasoningEffort::Medium),
                        ..ProviderModelInfo::default()
                    },
                )]),
                enabled: true,
            },
            Some("openrouter/qwen/qwen3".to_string()),
            None,
            Some("sk-test".to_string()),
        )
        .expect("upsert provider");

    let user_config = std::fs::read_to_string(fixture.home.join("custom-providers.json"))
        .expect("provider config");
    let workspace_config = fixture
        .workspace
        .as_ref()
        .expect("workspace")
        .join(".devo")
        .join("config.toml");
    let document: serde_json::Value =
        serde_json::from_str(&user_config).expect("parse provider config");

    assert!(user_config.contains("\"openrouter\""));
    assert!(user_config.contains("\"qwen/qwen3\""));
    assert!(
        document.get("model").is_none(),
        "session default must not live in custom-providers.json"
    );
    let config_toml =
        std::fs::read_to_string(fixture.home.join("config.toml")).expect("config.toml");
    assert!(
        config_toml.contains("openrouter/qwen/qwen3"),
        "expected model in config.toml, got:\n{config_toml}"
    );
    assert_eq!(
        document["provider"]["openrouter"]["headers"]["X-Devo"].as_str(),
        Some("yes")
    );
    assert_eq!(
        document["provider"]["openrouter"]["credential"].as_str(),
        Some("openrouter")
    );
    assert!(document["provider"]["openrouter"].get("options").is_none());
    assert_eq!(written_provider.credential.as_deref(), Some("openrouter"));
    assert_eq!(
        written_provider.headers,
        BTreeMap::from([("X-Devo".to_string(), "yes".to_string())])
    );
    assert_eq!(
        store.provider_connections().expect("list connections")[0].headers,
        BTreeMap::from([("X-Devo".to_string(), "yes".to_string())])
    );
    assert!(!workspace_config.exists());
    let auth_config = std::fs::read_to_string(fixture.home.join("auth.json")).expect("auth config");
    let auth_document: serde_json::Value =
        serde_json::from_str(&auth_config).expect("parse auth config");
    assert_eq!(
        auth_document["openrouter"]["type"].as_str(),
        Some("api_key")
    );
    assert_eq!(auth_document["openrouter"]["key"].as_str(), Some("sk-test"));
}

#[test]
fn provider_upsert_migrates_legacy_model_name_to_request_model() {
    let fixture = ConfigFixture::new("provider-upsert-existing-binding");
    fixture.write_home(
        r#"
[defaults]
model_binding = "deepseek-v4-flash-deepseek"

[providers.Deepseek]
base_url = "https://api.deepseek.com"
credential = "deepseek_api_key"
enabled = true
name = "Deepseek"
wire_apis = ["openai_chat_completions"]

[model_bindings.deepseek-v4-flash-deepseek]
display_name = "deepseek-v4-flash"
enabled = true
invocation_method = "openai_chat_completions"
model_name = "deepseek-v4-flash"
custom_binding_key = "preserved"
model_slug = "deepseek-v4-flash"
provider = "Deepseek"
"#,
    );

    let mut store =
        AppConfigStore::load(fixture.home.clone(), /*workspace_root*/ None).expect("load store");
    store
        .upsert_provider_connection(
            ProviderInfo {
                id: "Deepseek".to_string(),
                name: "Deepseek".to_string(),
                description: None,
                base_url: Some("https://api.deepseek.com".to_string()),
                credential: Some("deepseek_api_key".to_string()),
                headers: BTreeMap::new(),
                options: None,
                request: None,
                compat: None,
                model_overrides: BTreeMap::new(),
                wire_apis: vec![ProviderWireApi::OpenAIChatCompletions],
                models: BTreeMap::from([(
                    "DeepSeek-V4-Flash".to_string(),
                    ProviderModelInfo {
                        name: Some("DeepSeek-V4-Flash".to_string()),
                        wire_api: Some(ProviderWireApi::OpenAIChatCompletions),
                        ..ProviderModelInfo::default()
                    },
                )]),
                enabled: true,
            },
            Some("Deepseek/DeepSeek-V4-Flash".to_string()),
            None,
            /*api_key*/ None,
        )
        .expect("upsert provider");

    let user_config = std::fs::read_to_string(fixture.home.join("custom-providers.json"))
        .expect("provider config");
    let document: serde_json::Value =
        serde_json::from_str(&user_config).expect("parse provider config");
    let provider = &document["provider"]["Deepseek"];
    let model = &provider["models"]["DeepSeek-V4-Flash"];

    assert_eq!(
        provider["base_url"].as_str(),
        Some("https://api.deepseek.com")
    );
    assert_eq!(model["name"].as_str(), Some("DeepSeek-V4-Flash"));
    assert_eq!(model["wire_api"].as_str(), Some("openai_chat_completions"));
    assert_eq!(
        document["model"].as_str(),
        None,
        "session default must not live in custom-providers.json"
    );
    let legacy_config =
        std::fs::read_to_string(fixture.home.join("config.toml")).expect("legacy config");
    let legacy_document: toml::Value = toml::from_str(&legacy_config).expect("parse legacy config");
    assert!(legacy_document.get("providers").is_none());
    assert!(legacy_document.get("model_bindings").is_none());
    assert_eq!(
        legacy_document.get("model").and_then(|v| v.as_str()),
        Some("Deepseek/DeepSeek-V4-Flash")
    );
}

#[test]
fn loader_rejects_invalid_config_values() {
    let cases = [
        (
            "config-validation",
            "[logging.file]\nfilename_prefix = '   '\n",
        ),
        (
            "config-skill-roots",
            "[skills]\nuser_roots = ['skills', 'skills']\n",
        ),
        (
            "config-update-interval",
            "[updates]\ncheck_interval_hours = 0\n",
        ),
    ];
    for (name, config) in cases {
        let fixture = ConfigFixture::new(name);
        fixture.write_home(config);
        assert!(matches!(
            fixture.load_err(),
            super::AppConfigError::Validation { .. }
        ));
    }
}

#[test]
fn loader_reads_project_configs() {
    let cases = [
        (
            "config-projects",
            "[projects.\"C:\\\\repo\"]\npermission_preset = 'auto-review'\n",
            ProjectConfig {
                permission_preset: Some(PermissionPreset::AutoReview),
                sandbox_profile: None,
            },
        ),
        (
            "config-projects-sandbox",
            "[projects.\"C:\\\\repo\"]\nsandbox_profile = 'strict'\n",
            ProjectConfig {
                permission_preset: None,
                sandbox_profile: Some("strict".to_string()),
            },
        ),
        (
            "config-legacy-read-only",
            "[projects.\"C:\\\\repo\"]\npermission_preset = 'read-only'\n",
            ProjectConfig {
                permission_preset: Some(PermissionPreset::Default),
                sandbox_profile: None,
            },
        ),
    ];
    for (name, config, expected) in cases {
        let fixture = ConfigFixture::new(name);
        fixture.write_home(config);
        assert_eq!(
            fixture.load().projects,
            BTreeMap::from([("C:\\repo".to_string(), expected)])
        );
    }
}

#[test]
fn mcp_upsert_remove_enable_round_trip_preserves_unrelated_sections() {
    let fixture = ConfigFixture::new("mcp-upsert-roundtrip");
    fixture.write_home(
        r#"
[updates]
enabled = true
check_on_startup = false
check_interval_hours = 12

[logging]
level = "warn"
"#,
    );
    let mut store =
        AppConfigStore::load(fixture.home.clone(), /*workspace_root*/ None).expect("load store");
    let stdio_record = mcp_record(
        "time",
        McpTransportConfig::Stdio {
            command: vec![
                "docker".into(),
                "run".into(),
                "-i".into(),
                "--rm".into(),
                "mcp/time".into(),
            ],
            cwd: None,
            env: BTreeMap::new(),
            env_vars: Vec::new(),
        },
    );
    store
        .upsert_mcp_server(stdio_record.clone())
        .expect("upsert stdio");
    store
        .upsert_mcp_server(mcp_record(
            "hello",
            McpTransportConfig::StreamableHttp {
                url: "http://localhost:8080/mcp".into(),
                auth: None,
                http_headers: BTreeMap::new(),
                env_http_headers: BTreeMap::new(),
            },
        ))
        .expect("upsert http");
    store
        .upsert_mcp_server(mcp_record(
            "legacy",
            McpTransportConfig::Sse {
                url: "https://example.com/mcp/sse".into(),
                auth: None,
                http_headers: BTreeMap::new(),
                env_http_headers: BTreeMap::new(),
            },
        ))
        .expect("upsert sse");

    let user_config =
        std::fs::read_to_string(fixture.home.join("config.toml")).expect("read user config");
    assert!(user_config.contains("check_on_startup"));
    assert!(user_config.contains("level"));
    let server_ids: Vec<&str> = store
        .mcp_servers()
        .iter()
        .map(|server| server.id.0.as_str())
        .collect();
    assert!(server_ids.contains(&"time"));
    assert!(server_ids.contains(&"hello"));
    assert!(server_ids.contains(&"legacy"));
    assert!(server_ids.contains(&super::BUNDLED_CODE_SEARCH_MCP_SERVER_ID));
    assert_eq!(
        store
            .mcp_servers()
            .iter()
            .find(|server| server.id.0 == "time")
            .expect("time server"),
        &stdio_record
    );

    store
        .set_mcp_server_enabled("time", /*enabled*/ false)
        .expect("disable");
    assert!(
        !store
            .mcp_servers()
            .iter()
            .find(|server| server.id.0 == "time")
            .expect("time server")
            .enabled
    );

    store.remove_mcp_server("hello").expect("remove hello");
    assert!(
        store
            .mcp_servers()
            .iter()
            .all(|server| server.id.0 != "hello")
    );

    let reloaded =
        AppConfigStore::load(fixture.home.clone(), /*workspace_root*/ None).expect("reload");
    let reloaded_ids: Vec<&str> = reloaded
        .mcp_servers()
        .iter()
        .map(|server| server.id.0.as_str())
        .collect();
    assert!(reloaded_ids.contains(&"time"));
    assert!(reloaded_ids.contains(&"legacy"));
    assert!(reloaded_ids.contains(&super::BUNDLED_CODE_SEARCH_MCP_SERVER_ID));
    assert!(!reloaded_ids.contains(&"hello"));
    assert!(
        !reloaded
            .mcp_servers()
            .iter()
            .find(|server| server.id.0 == "time")
            .expect("time server")
            .enabled
    );
}

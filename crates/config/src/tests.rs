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
use super::LoggingConfig;
use super::ModelBindingConfig;
use super::OAuthCredentialsStoreMode;
use super::ProjectConfig;
use super::ProviderConfigSection;
use super::ProviderDefaultsConfig;
use super::ProviderHttpConfig;
use super::ProviderVendorConfig;
use super::ResearchConfig;
use super::SummaryModelSelection;
use super::ToolsConfig;
use super::UpdatesConfig;
use crate::BundledSkillsConfig;
use crate::SkillsConfig;
use devo_protocol::ProviderModelBinding;
use devo_protocol::ProviderVendor;
use devo_protocol::ProviderWireApi;

fn unique_temp_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("devo-{name}-{nanos}"));
    std::fs::create_dir_all(&path).expect("create temp dir");
    path
}

#[test]
fn loader_merges_user_project_and_cli_layers() {
    let root = unique_temp_dir("config-merge");
    let home = root.join("home").join(".devo");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::create_dir_all(workspace.join(".devo")).expect("workspace config dir");

    std::fs::write(
        home.join("config.toml"),
        "default_model = 'ignored'\n[anthropic]\nmodel = 'also-ignored'\n[context]\npreserve_recent_turns = 5\n[logging]\nlevel = 'debug'\n[logging.file]\nmax_files = 30\n",
    )
    .expect("write user config");
    std::fs::write(
        workspace.join(".devo").join("config.toml"),
        "enable_auxiliary_model = true\nproject_root_markers = ['.git', 'Cargo.toml']\n[context]\nauto_compact_percent = 80\n[logging]\njson = true\n[logging.file]\ndirectory = 'diagnostics'\nfilename_prefix = 'agent'\n[skills]\nenabled = true\nworkspace_roots = ['project-skills']\nwatch_for_changes = false\n",
    )
    .expect("write project config");

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

    let loader = FileSystemAppConfigLoader::new(home).with_cli_overrides(cli_overrides);
    let config = loader.load(Some(&workspace)).expect("load config");

    assert_eq!(
        config,
        AppConfig {
            summary_model: SummaryModelSelection::UseAxiliaryModel,
            server: super::ServerConfig {
                listen: vec!["stdio://".into()],
                max_connections: 32,
                event_buffer_size: 1024,
                idle_session_timeout_secs: 1800,
                persist_ephemeral_sessions: false,
                auth: Default::default(),
            },
            logging: LoggingConfig {
                level: "trace".into(),
                json: true,
                redact_secrets_in_logs: true,
                file: super::LoggingFileConfig {
                    directory: Some(PathBuf::from("cli-logs")),
                    filename_prefix: "agent".into(),
                    rotation: LogRotation::Hourly,
                    max_files: 2,
                },
            },
            skills: SkillsConfig {
                enabled: false,
                user_roots: vec![PathBuf::from("custom-user-skills")],
                workspace_roots: vec![PathBuf::from("project-skills")],
                watch_for_changes: false,
                bundled: Some(BundledSkillsConfig { enabled: true }),
                include_instructions: Some(true),
                config: Vec::new(),
            },
            experimental: ExperimentalConfig { code_search: true },
            mcp_oauth_credentials_store: Some(OAuthCredentialsStoreMode::default()),
            mcp: super::McpConfig::default(),
            tools: ToolsConfig::default(),
            hooks: HooksConfig::default(),
            provider: ProviderConfigSection::default(),
            provider_http: super::ProviderHttpConfig::default(),
            research: ResearchConfig::default(),
            updates: UpdatesConfig {
                enabled: false,
                check_on_startup: true,
                check_interval_hours: 48,
            },
            project_root_markers: vec![".workspace".into()],
            projects: BTreeMap::new(),
        }
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn default_app_config_enables_code_search() {
    assert_eq!(
        AppConfig::default().experimental,
        ExperimentalConfig { code_search: true }
    );
}

#[test]
fn default_app_config_disables_server_auth() {
    assert_eq!(
        AppConfig::default().server.auth,
        super::ServerAuthConfig {
            enabled: false,
            method_id: "agent-login".to_string(),
            name: "Agent login".to_string(),
            description: None,
            logout: true,
        }
    );
}

#[test]
fn loader_reads_server_auth_config() {
    let root = unique_temp_dir("config-server-auth");
    let home = root.join("home").join(".devo");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::write(
        home.join("config.toml"),
        r#"
[server.auth]
enabled = true
method_id = "company-login"
name = "Company login"
description = "Sign in with company credentials"
logout = false
"#,
    )
    .expect("write user config");

    let loader = FileSystemAppConfigLoader::new(home);
    let config = loader.load(None).expect("load config");

    assert_eq!(
        config.server.auth,
        super::ServerAuthConfig {
            enabled: true,
            method_id: "company-login".to_string(),
            name: "Company login".to_string(),
            description: Some("Sign in with company credentials".to_string()),
            logout: false,
        }
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn loader_rejects_empty_server_auth_method_id_when_enabled() {
    let root = unique_temp_dir("config-server-auth-empty-method");
    let home = root.join("home").join(".devo");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::write(
        home.join("config.toml"),
        "[server.auth]\nenabled = true\nmethod_id = '   '\n",
    )
    .expect("write user config");

    let loader = FileSystemAppConfigLoader::new(home);
    let result = loader.load(None);

    match result {
        Err(super::AppConfigError::Validation { message }) => assert_eq!(
            message,
            "server.auth.method_id must not be empty when server auth is enabled"
        ),
        other => panic!("expected server auth validation error, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn loader_rejects_empty_server_auth_name_when_enabled() {
    let root = unique_temp_dir("config-server-auth-empty-name");
    let home = root.join("home").join(".devo");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::write(
        home.join("config.toml"),
        "[server.auth]\nenabled = true\nname = '   '\n",
    )
    .expect("write user config");

    let loader = FileSystemAppConfigLoader::new(home);
    let result = loader.load(None);

    match result {
        Err(super::AppConfigError::Validation { message }) => assert_eq!(
            message,
            "server.auth.name must not be empty when server auth is enabled"
        ),
        other => panic!("expected server auth validation error, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn loader_accepts_experimental_code_search_kebab_key() {
    let root = unique_temp_dir("config-experimental-kebab");
    let home = root.join("home").join(".devo");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::write(
        home.join("config.toml"),
        "[experimental]\ncode-search = true\n",
    )
    .expect("write user config");

    let loader = FileSystemAppConfigLoader::new(home);
    let config = loader.load(None).expect("load config");

    assert_eq!(
        config.experimental,
        ExperimentalConfig { code_search: true }
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn loader_accepts_experimental_code_search_snake_alias() {
    let root = unique_temp_dir("config-experimental-snake");
    let home = root.join("home").join(".devo");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::write(
        home.join("config.toml"),
        "[experimental]\ncode_search = true\n",
    )
    .expect("write user config");

    let loader = FileSystemAppConfigLoader::new(home);
    let config = loader.load(None).expect("load config");

    assert_eq!(
        config.experimental,
        ExperimentalConfig { code_search: true }
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn loader_merges_experimental_config_in_normal_precedence_order() {
    let root = unique_temp_dir("config-experimental-merge");
    let home = root.join("home").join(".devo");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::create_dir_all(workspace.join(".devo")).expect("workspace config dir");
    std::fs::write(
        home.join("config.toml"),
        "[experimental]\ncode-search = false\n",
    )
    .expect("write user config");
    std::fs::write(
        workspace.join(".devo").join("config.toml"),
        "[experimental]\ncode-search = true\n",
    )
    .expect("write project config");
    let cli_overrides: toml::Value = "[experimental]\ncode-search = false\n"
        .parse()
        .expect("parse cli overrides");

    let loader = FileSystemAppConfigLoader::new(home).with_cli_overrides(cli_overrides);
    let config = loader.load(Some(&workspace)).expect("load config");

    assert_eq!(
        config.experimental,
        ExperimentalConfig { code_search: false }
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn loader_reads_hook_command_config() {
    let root = unique_temp_dir("config-hooks");
    let home = root.join("home").join(".devo");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::write(
        home.join("config.toml"),
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
    )
    .expect("write user config");

    let loader = FileSystemAppConfigLoader::new(home);
    let config = loader.load(None).expect("load config");

    assert_eq!(
        config.hooks,
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

    let _ = std::fs::remove_dir_all(root);
}

/// Trace: L2-DES-APP-005
/// Verifies: provider HTTP proxy settings and provider header fields follow user/workspace merge precedence.
#[test]
fn loader_merges_provider_sections_with_provider_overlay_rules() {
    let root = unique_temp_dir("config-provider-merge");
    let home = root.join("home").join(".devo");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::create_dir_all(workspace.join(".devo")).expect("workspace config dir");

    std::fs::write(
        home.join("config.toml"),
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
model_name = "user/model"
invocation_method = "openai_responses"
"#,
    )
    .expect("write user config");
    std::fs::write(
        workspace.join(".devo").join("config.toml"),
        r#"
[provider_http]
proxy_url = "http://workspace-proxy.example:8080"

[providers.main]
name = "Project Provider"

[model_bindings.main]
model_slug = "project-model"
provider = "main"
model_name = "project/model"
invocation_method = "openai_responses"
"#,
    )
    .expect("write project config");

    let loader = FileSystemAppConfigLoader::new(home);
    let config = loader.load(Some(&workspace)).expect("load config");

    assert_eq!(
        config.provider_http,
        ProviderHttpConfig {
            proxy_url: Some("http://workspace-proxy.example:8080".to_string()),
            no_proxy: None,
        }
    );
    assert_eq!(
        config.provider,
        ProviderConfigSection {
            defaults: ProviderDefaultsConfig {
                model_binding: Some("main".to_string()),
            },
            providers: BTreeMap::from([(
                "main".to_string(),
                ProviderVendorConfig {
                    name: "Project Provider".to_string(),
                    base_url: Some("https://user.example/v1".to_string()),
                    credential: Some("user_api_key".to_string()),
                    headers: Some(r#"{"X-User":"yes"}"#.to_string()),
                    wire_apis: vec![ProviderWireApi::OpenAIResponses],
                    web_search: None,
                    web_fetch: None,
                    enabled: true,
                },
            )]),
            model_bindings: BTreeMap::from([(
                "main".to_string(),
                ModelBindingConfig {
                    model_slug: "project-model".to_string(),
                    provider: "main".to_string(),
                    model_name: "project/model".to_string(),
                    invocation_method: ProviderWireApi::OpenAIResponses,
                    ..ModelBindingConfig::default()
                },
            )]),
            ..ProviderConfigSection::default()
        }
    );

    let _ = std::fs::remove_dir_all(root);
}

/// Trace: L2-DES-APP-005
/// Verifies: omitted defaulted provider fields in a higher-priority partial overlay do not overwrite lower-priority values.
#[test]
fn loader_provider_overlay_preserves_absent_defaulted_provider_fields() {
    let root = unique_temp_dir("config-provider-defaulted-overlay");
    let home = root.join("home").join(".devo");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::create_dir_all(workspace.join(".devo")).expect("workspace config dir");

    std::fs::write(
        home.join("config.toml"),
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
model_name = "user/model"
invocation_method = "openai_responses"
enabled = false
"#,
    )
    .expect("write user config");
    std::fs::write(
        workspace.join(".devo").join("config.toml"),
        r#"
[providers.main]
name = "Project Provider"

[model_bindings.main]
model_slug = "project-model"
provider = "main"
model_name = "project/model"
"#,
    )
    .expect("write project config");

    let loader = FileSystemAppConfigLoader::new(home);
    let config = loader.load(Some(&workspace)).expect("load config");

    assert_eq!(
        config.provider,
        ProviderConfigSection {
            defaults: ProviderDefaultsConfig {
                model_binding: Some("main".to_string()),
            },
            providers: BTreeMap::from([(
                "main".to_string(),
                ProviderVendorConfig {
                    name: "Project Provider".to_string(),
                    base_url: Some("https://user.example/v1".to_string()),
                    credential: Some("user_api_key".to_string()),
                    headers: Some(r#"{"X-User":"yes"}"#.to_string()),
                    wire_apis: vec![ProviderWireApi::OpenAIResponses],
                    web_search: None,
                    web_fetch: None,
                    enabled: false,
                },
            )]),
            model_bindings: BTreeMap::from([(
                "main".to_string(),
                ModelBindingConfig {
                    model_slug: "project-model".to_string(),
                    provider: "main".to_string(),
                    model_name: "project/model".to_string(),
                    invocation_method: ProviderWireApi::OpenAIResponses,
                    enabled: false,
                    ..ModelBindingConfig::default()
                },
            )]),
            ..ProviderConfigSection::default()
        }
    );

    let _ = std::fs::remove_dir_all(root);
}

/// Trace: L2-DES-APP-005
/// Verifies: CLI provider overrides participate in the same provider merge precedence as other CLI config.
#[test]
fn loader_applies_cli_provider_overrides_to_provider_section() {
    let root = unique_temp_dir("config-provider-cli-overlay");
    let home = root.join("home").join(".devo");
    std::fs::create_dir_all(&home).expect("home config dir");

    std::fs::write(
        home.join("config.toml"),
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
model_name = "user/model"
invocation_method = "openai_responses"
"#,
    )
    .expect("write user config");
    let cli_overrides: toml::Value = r#"
[providers.main]
name = "CLI Provider"
enabled = false

[model_bindings.main]
model_slug = "cli-model"
provider = "main"
model_name = "cli/model"
invocation_method = "openai_responses"
enabled = false
"#
    .parse()
    .expect("parse cli overrides");

    let loader = FileSystemAppConfigLoader::new(home).with_cli_overrides(cli_overrides);
    let config = loader.load(None).expect("load config");

    assert_eq!(
        config.provider,
        ProviderConfigSection {
            defaults: ProviderDefaultsConfig {
                model_binding: Some("main".to_string()),
            },
            providers: BTreeMap::from([(
                "main".to_string(),
                ProviderVendorConfig {
                    name: "CLI Provider".to_string(),
                    base_url: Some("https://user.example/v1".to_string()),
                    credential: Some("user_api_key".to_string()),
                    headers: None,
                    wire_apis: vec![ProviderWireApi::OpenAIResponses],
                    web_search: None,
                    web_fetch: None,
                    enabled: false,
                },
            )]),
            model_bindings: BTreeMap::from([(
                "main".to_string(),
                ModelBindingConfig {
                    model_slug: "cli-model".to_string(),
                    provider: "main".to_string(),
                    model_name: "cli/model".to_string(),
                    invocation_method: ProviderWireApi::OpenAIResponses,
                    enabled: false,
                    ..ModelBindingConfig::default()
                },
            )]),
            ..ProviderConfigSection::default()
        }
    );

    let _ = std::fs::remove_dir_all(root);
}

/// Trace: L2-DES-APP-005
/// Verifies: provider upsert persists custom provider header JSON in user config and projections.
#[test]
fn provider_upsert_writes_user_config_when_workspace_is_active() {
    let root = unique_temp_dir("provider-upsert-user");
    let home = root.join("home").join(".devo");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::create_dir_all(workspace.join(".devo")).expect("workspace config dir");

    let mut store = AppConfigStore::load(home.clone(), Some(&workspace)).expect("load store");
    let written_provider = store
        .upsert_provider_vendor(
            "openrouter".to_string(),
            ProviderVendor {
                name: "openrouter".to_string(),
                base_url: Some("https://openrouter.ai/api/v1".to_string()),
                credential: None,
                headers: Some(r#"{"X-Devo":"yes"}"#.to_string()),
                wire_apis: vec![ProviderWireApi::OpenAIChatCompletions],
                enabled: true,
            },
            Some(ProviderModelBinding {
                binding_id: "qwen-openrouter".to_string(),
                model_slug: "qwen".to_string(),
                provider: "openrouter".to_string(),
                model_name: "qwen/qwen3".to_string(),
                display_name: Some("Qwen".to_string()),
                invocation_method: ProviderWireApi::OpenAIChatCompletions,
                default_reasoning_effort: Some("medium".to_string()),
                enabled: true,
            }),
            Some("qwen-openrouter".to_string()),
            Some("sk-test".to_string()),
        )
        .expect("upsert provider");

    let user_config = std::fs::read_to_string(home.join("config.toml")).expect("user config");
    let workspace_config = workspace.join(".devo").join("config.toml");
    let document: toml::Value = toml::from_str(&user_config).expect("parse user config");

    assert!(user_config.contains("[providers.openrouter]"));
    assert!(user_config.contains("[model_bindings.qwen-openrouter]"));
    assert!(user_config.contains("model_binding = \"qwen-openrouter\""));
    assert_eq!(
        document["providers"]["openrouter"]["headers"].as_str(),
        Some(r#"{"X-Devo":"yes"}"#)
    );
    assert_eq!(
        written_provider.headers,
        Some(r#"{"X-Devo":"yes"}"#.to_string())
    );
    assert_eq!(
        store.provider_vendors()[0].headers,
        Some(r#"{"X-Devo":"yes"}"#.to_string())
    );
    assert!(!workspace_config.exists());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn provider_upsert_updates_existing_binding_model_name() {
    let root = unique_temp_dir("provider-upsert-existing-binding");
    let home = root.join("home").join(".devo");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::write(
        home.join("config.toml"),
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
model_slug = "deepseek-v4-flash"
provider = "Deepseek"
"#,
    )
    .expect("write user config");

    let mut store =
        AppConfigStore::load(home.clone(), /*workspace_root*/ None).expect("load store");
    store
        .upsert_provider_vendor(
            "Deepseek".to_string(),
            ProviderVendor {
                name: "Deepseek".to_string(),
                base_url: Some("https://api.deepseek.com".to_string()),
                credential: Some("deepseek_api_key".to_string()),
                headers: None,
                wire_apis: vec![ProviderWireApi::OpenAIChatCompletions],
                enabled: true,
            },
            Some(ProviderModelBinding {
                binding_id: "deepseek-v4-flash-deepseek".to_string(),
                model_slug: "deepseek-v4-flash".to_string(),
                provider: "Deepseek".to_string(),
                model_name: "DeepSeek-V4-Flash".to_string(),
                display_name: Some("DeepSeek-V4-Flash".to_string()),
                invocation_method: ProviderWireApi::OpenAIChatCompletions,
                default_reasoning_effort: None,
                enabled: true,
            }),
            Some("deepseek-v4-flash-deepseek".to_string()),
            /*api_key*/ None,
        )
        .expect("upsert provider");

    let user_config = std::fs::read_to_string(home.join("config.toml")).expect("user config");
    let document: toml::Value = toml::from_str(&user_config).expect("parse user config");
    let binding = &document["model_bindings"]["deepseek-v4-flash-deepseek"];

    assert_eq!(binding["model_slug"].as_str(), Some("deepseek-v4-flash"));
    assert_eq!(binding["model_name"].as_str(), Some("DeepSeek-V4-Flash"));
    assert_eq!(binding["display_name"].as_str(), Some("DeepSeek-V4-Flash"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn loader_rejects_invalid_logging_file_prefix() {
    let root = unique_temp_dir("config-validation");
    let home = root.join("home").join(".devo");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::write(
        home.join("config.toml"),
        "[logging.file]\nfilename_prefix = '   '\n",
    )
    .expect("write user config");

    let loader = FileSystemAppConfigLoader::new(home);
    let result = loader.load(None);

    assert!(matches!(
        result,
        Err(super::AppConfigError::Validation { .. })
    ));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn loader_rejects_duplicate_skill_roots() {
    let root = unique_temp_dir("config-skill-roots");
    let home = root.join("home").join(".devo");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::write(
        home.join("config.toml"),
        "[skills]\nuser_roots = ['skills', 'skills']\n",
    )
    .expect("write user config");

    let loader = FileSystemAppConfigLoader::new(home);
    let result = loader.load(None);

    assert!(matches!(
        result,
        Err(super::AppConfigError::Validation { .. })
    ));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn loader_reads_project_configs() {
    let root = unique_temp_dir("config-projects");
    let home = root.join("home").join(".devo");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::write(
        home.join("config.toml"),
        "[projects.\"C:\\\\repo\"]\npermission_preset = 'read-only'\n",
    )
    .expect("write user config");

    let loader = FileSystemAppConfigLoader::new(home);
    let config = loader.load(None).expect("load config");

    assert_eq!(
        config.projects,
        BTreeMap::from([(
            "C:\\repo".to_string(),
            ProjectConfig {
                permission_preset: Some(PermissionPreset::ReadOnly),
            },
        )])
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn default_app_config_enables_startup_update_checks() {
    assert_eq!(
        AppConfig::default().updates,
        UpdatesConfig {
            enabled: true,
            check_on_startup: true,
            check_interval_hours: 24,
        }
    );
}

#[test]
fn loader_rejects_invalid_update_check_interval() {
    let root = unique_temp_dir("config-update-interval");
    let home = root.join("home").join(".devo");
    std::fs::create_dir_all(&home).expect("home config dir");
    std::fs::write(
        home.join("config.toml"),
        "[updates]\ncheck_interval_hours = 0\n",
    )
    .expect("write user config");

    let loader = FileSystemAppConfigLoader::new(home);
    let result = loader.load(None);

    assert!(matches!(
        result,
        Err(super::AppConfigError::Validation { .. })
    ));

    let _ = std::fs::remove_dir_all(root);
}

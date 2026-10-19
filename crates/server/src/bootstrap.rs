use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use clap::ValueEnum;
use devo_core::AgentsMdConfig;
use devo_core::AppConfigStore;
use devo_core::FileSystemSkillCatalog;
use devo_core::ModelCatalog;
use devo_core::PresetModelCatalog;
use devo_core::tools::ToolPlanConfig;
use devo_core::tools::handlers;
use devo_mcp::manager::RmcpMcpManager;
use devo_util_paths::FileSystemConfigPathResolver;

use crate::ListenTarget;
use crate::ProtocolSet;
use crate::ServerRuntime;
use crate::db::Database;
use crate::execution::ServerRuntimeDependencies;
use crate::load_server_provider;
use crate::resolve_listen_targets;
use crate::run_listeners_with_internal_proxy;
use crate::singleton::ServerControlAction;
use crate::singleton::SingletonRole;
use crate::singleton::acquire_singleton_role;
use crate::singleton::run_server_control;
use crate::singleton::run_stdio_proxy;
use crate::transport::DEFAULT_WEBSOCKET_BIND_ADDRESS;
use crate::transport::InternalProxyControl;
use crate::transport::InternalProxyEndpoint;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ServerTransportMode {
    Config,
    Stdio,
    #[value(name = "websocket")]
    WebSocket,
}

/// Command-line arguments accepted by the standalone server process entrypoint.
#[derive(Debug, Clone, PartialEq, Eq, Parser)]
#[command(name = "devo-server", version, about)]
pub struct ServerProcessArgs {
    /// Override the transport mode used by this server process.
    #[arg(long, value_enum, hide = true, default_value_t = ServerTransportMode::Config)]
    pub transport: ServerTransportMode,

    /// Protocol adapters exposed by this server process.
    #[arg(long, default_value = "native")]
    pub protocols: ProtocolSet,

    /// Print status for an existing singleton server and exit.
    #[arg(long, hide = true)]
    pub status: bool,

    /// Ask an existing singleton server to shut down and exit.
    #[arg(long, hide = true)]
    pub shutdown: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerProcessAction {
    Run,
    Status,
    Shutdown,
}

impl ServerProcessArgs {
    fn action(&self) -> Result<ServerProcessAction> {
        match (self.status, self.shutdown) {
            (false, false) => Ok(ServerProcessAction::Run),
            (true, false) => Ok(ServerProcessAction::Status),
            (false, true) => Ok(ServerProcessAction::Shutdown),
            (true, true) => anyhow::bail!("--status and --shutdown cannot be used together"),
        }
    }
}

#[derive(Default)]
pub struct ServerProcessRunOptions {
    pub external_shutdown: Option<tokio_util::sync::CancellationToken>,
}

/// Starts the transport-facing server runtime using the resolved application
/// configuration and listener set.
///
/// ## Singleton server (`singleton.rs`)
///
/// Devo allows at most **one real server process** per `DEVO_HOME`. Coordination
/// uses a file lock (`server.lock`) plus metadata (`server.lock.json`) that
/// records pid, a loopback WebSocket endpoint, and an auth token.
///
/// - **`SingletonRole::Real`**: this process acquired the lock and becomes the
///   sole server. It binds an internal proxy listener, writes metadata, and runs
///   until shutdown.
/// - **`SingletonRole::Proxy`**: another process already holds the lock. This
///   process does not start a second runtime: stdio mode forwards to the
///   existing server via `run_stdio_proxy`; `--status` / `--shutdown` talk to
///   the internal control channel on that server.
///
/// ## Internal proxy (`run_listeners_with_internal_proxy`)
///
/// The real server exposes an extra **loopback-only** WebSocket listener
/// (`127.0.0.1:0`, ephemeral port). It is used for:
///
/// 1. **Stdio proxy clients** — a second `devo server --transport stdio` connects
///    here and pipes stdin/stdout through WebSocket frames (see `run_stdio_proxy`).
/// 2. **Control plane** — `devo server --status` / `--shutdown` send
///    `server/status` or `server/shutdown` after token auth.
///
/// The published `endpoint` in `server.lock.json` is this internal proxy URL, not
/// the public config WebSocket address.
pub async fn run_server_process(
    args: ServerProcessArgs,
    options: ServerProcessRunOptions,
) -> Result<()> {
    let resolver = FileSystemConfigPathResolver::from_env()?;
    let action = args.action()?;
    // Decide whether this process is the one true server or a lightweight proxy/
    // control client. Lock file lives under DEVO_HOME (see singleton.rs).
    let singleton_role = acquire_singleton_role(&resolver.user_config_dir())?;
    let real_server_guard = match singleton_role {
        SingletonRole::Real(guard) => match action {
            ServerProcessAction::Run => guard,
            ServerProcessAction::Status | ServerProcessAction::Shutdown => {
                // We hold the lock but were not asked to run — no metadata file yet.
                println!("devo server is not running");
                return Ok(());
            }
        },
        SingletonRole::Proxy(metadata) => match action {
            ServerProcessAction::Run => {
                let result = run_server_control(
                    &metadata,
                    ServerControlAction::EnableProtocols(args.protocols.clone()),
                )
                .await?;
                if args.transport == ServerTransportMode::Stdio {
                    tracing::info!(
                        pid = metadata.pid,
                        endpoint = %metadata.endpoint,
                        protocols = %result
                            .enabled_protocols
                            .as_ref()
                            .unwrap_or(&args.protocols),
                        "proxying stdio to existing singleton server"
                    );
                    return run_stdio_proxy(metadata).await;
                }
                print_existing_server_status(
                    &metadata,
                    "already running",
                    result.enabled_protocols.as_ref(),
                );
                return Ok(());
            }
            // `--status` / `--shutdown`: one-shot WebSocket control, then exit.
            ServerProcessAction::Status => {
                let result = run_server_control(&metadata, ServerControlAction::Status).await?;
                print_existing_server_status(
                    &metadata,
                    result.status.as_str(),
                    result.enabled_protocols.as_ref(),
                );
                return Ok(());
            }
            ServerProcessAction::Shutdown => {
                let result = run_server_control(&metadata, ServerControlAction::Shutdown).await?;
                print_existing_server_status(
                    &metadata,
                    result.status.as_str(),
                    result.enabled_protocols.as_ref(),
                );
                return Ok(());
            }
        },
    };
    // Real server: bind ephemeral loopback WS for stdio-proxy + control clients.
    let internal_proxy = InternalProxyEndpoint::bind().await?;
    // Persist ws://127.0.0.1:<port> + random token into server.lock.json so
    // proxy/control processes know where and how to connect.
    let singleton_metadata =
        real_server_guard.publish_endpoint(internal_proxy.endpoint().to_string())?;

    if let Err(error) =
        devo_core::migrate_session_defaults_to_config_toml(&resolver.user_config_dir())
    {
        tracing::warn!(
            error = %error,
            "failed to migrate session defaults into config.toml"
        );
    }
    // Align on-disk providers.json with sparse overlay semantics before load.
    if let Err(error) =
        devo_core::migrate_user_provider_catalog_overlays(&resolver.user_config_dir())
    {
        tracing::warn!(
            error = %error,
            "failed to sparsify user provider catalog overlays"
        );
    }
    if let Ok(builtin) = devo_core::builtin_provider_config()
        && let Err(error) =
            devo_core::migrate_custom_providers_file(&resolver.user_config_dir(), &builtin)
    {
        tracing::warn!(
            error = %error,
            "failed to split custom providers into custom-providers.json"
        );
    }

    // Migrate legacy auth.json envelope → provider-keyed AuthStorage shape.
    let auth_path = resolver
        .user_config_dir()
        .join(devo_core::AUTH_CONFIG_FILE_NAME);
    if let Err(error) = devo_core::read_user_auth_config(&auth_path) {
        tracing::warn!(error = %error, "failed to migrate user auth.json");
    }

    let config_store = Arc::new(std::sync::Mutex::new(AppConfigStore::load(
        resolver.user_config_dir(),
        /*workspace_root*/ None,
    )?));
    let config = config_store
        .lock()
        .expect("app config store mutex should not be poisoned")
        .effective_config()
        .clone();
    let effective_listen = match args.transport {
        ServerTransportMode::Config => config.server.listen.clone(),
        ServerTransportMode::Stdio => vec!["stdio://".to_string()],
        ServerTransportMode::WebSocket => {
            vec![format!("ws://{DEFAULT_WEBSOCKET_BIND_ADDRESS}")]
        }
    };
    let listen_targets = resolve_listen_targets(&effective_listen)?;
    let effective_listen = listen_targets
        .iter()
        .map(|target| match target {
            ListenTarget::Stdio => "stdio://".to_string(),
            ListenTarget::WebSocket { bind_address } => format!("ws://{bind_address}"),
        })
        .collect::<Vec<_>>();

    tracing::info!(
        user_config = %resolver.user_config_file().display(),
        configured_listen = ?config.server.listen,
        effective_listen = ?effective_listen,
        max_connections = config.server.max_connections,
        protocols = %args.protocols,
        "loaded server config"
    );

    let mcp_manager: Arc<dyn devo_core::McpManager> = Arc::new(RmcpMcpManager::new(
        merge_tui_settings_mcp_servers(config.mcp_runtime.clone(), &resolver.user_config_dir()),
        config.mcp_oauth_credentials_store.unwrap_or_default(),
    ));
    let tool_plan = ToolPlanConfig::from_app_config(&config);
    let registry =
        handlers::build_registry_from_plan_with_mcp(&tool_plan, Arc::clone(&mcp_manager)).await;
    let model_catalog: Arc<dyn ModelCatalog> =
        Arc::new(PresetModelCatalog::load_from_provider_config_with_home(
            &config.provider_catalog_config(),
            &config.provider.model_overrides,
            Some(resolver.user_config_dir().as_path()),
        )?);
    let default_model = model_catalog.resolve_for_turn(None)?.slug.clone();
    if !config.has_provider_configuration() {
        tracing::warn!(
            "No provider configured. Run `devo onboard` to complete setup; continuing with onboarding-capable server"
        );
    }
    let provider = load_server_provider(
        &config,
        Some(default_model.as_str()),
        &resolver.user_config_dir(),
    )
    .await?;
    let skill_catalog = Box::new(FileSystemSkillCatalog::with_devo_home(
        config.skills.clone(),
        resolver.user_config_dir(),
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
        config.project_root_markers.clone(),
    ));
    // Initialize SQLite database
    let db_path = resolver.user_config_dir().join("devo.db");
    tracing::info!(db_path = %db_path.display(), "opening database");
    let db = Arc::new(Database::open(db_path)?);

    let registry = Arc::new(registry);
    let provider_router = Arc::clone(&provider.provider_router);
    let process_context = Arc::new(crate::session_context::SessionRuntimeContext::from_parts(
        provider.provider,
        provider_router,
        Arc::clone(&registry),
        mcp_manager,
        provider.default_model,
        model_catalog,
        Arc::new(std::sync::Mutex::new(skill_catalog)),
        AgentsMdConfig {
            project_root_markers: config.project_root_markers.clone(),
            ..AgentsMdConfig::default()
        },
        config_store,
    ));
    let runtime = ServerRuntime::with_protocols(
        resolver.user_config_dir(),
        ServerRuntimeDependencies::new(process_context, db),
        args.protocols.clone(),
    );
    // Serve cached/embedded models immediately; network updates never delay startup.
    {
        let runtime = runtime.clone();
        let catalog = config.catalog.clone();
        tokio::spawn(async move {
            let outcome = runtime.refresh_remote_catalog(&catalog).await;
            if let devo_core::CatalogRefreshOutcome::Failed { stage, message } = outcome {
                tracing::warn!(?stage, error = %message, "catalog refresh failed; retaining cached models");
            }
        });
    }
    runtime
        .run_global_hook(
            devo_core::HookEvent::Setup,
            serde_json::Map::from_iter([("trigger".to_string(), serde_json::json!("init"))]),
        )
        .await;
    // Rebuild SQLite session index from on-disk SessionMeta headers. Always
    // refresh: empty DBs with pre-seeded rollouts (restore / stress corpora)
    // otherwise stay invisible to session/list and session/resume.
    {
        let rollout_store = runtime.rollout_store();
        let db = runtime.deps_db();
        tokio::task::spawn_blocking(move || match rollout_store.index_rollout_metadata(&db) {
            Ok(()) => tracing::info!("rollout metadata index refresh completed"),
            Err(error) => {
                tracing::warn!(%error, "rollout metadata index refresh failed");
            }
        });
    }
    // Delivery-log reconciliation (08 §7): backfill event_log rows a crash
    // prevented the append path from writing. Runs in the background;
    // session/list correctness never depends on it.
    {
        let rollout_store = runtime.rollout_store();
        let db = runtime.deps_db();
        tokio::task::spawn_blocking(move || {
            match crate::event_reconcile::reconcile_event_log(&rollout_store, &db) {
                Ok(stats) => {
                    if stats.rows_inserted > 0 || stats.files_damaged > 0 {
                        tracing::info!(
                            rows_inserted = stats.rows_inserted,
                            files_damaged = stats.files_damaged,
                            "event_log reconciliation completed"
                        );
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "event_log reconciliation failed");
                }
            }
        });
    }

    let shutdown_signal = tokio_util::sync::CancellationToken::new();
    let internal_proxy_control = InternalProxyControl::new(shutdown_signal.clone());
    let external_shutdown = options.external_shutdown.clone();

    // Concurrent listeners: configured stdio/ws targets + internal proxy task.
    // Returns when any listener exits; shutdown also via Ctrl+C, external token,
    // or internal-proxy `server/shutdown` (cancels shutdown_signal).
    tokio::select! {
        result = run_listeners_with_internal_proxy(
            runtime.clone(),
            &effective_listen,
            internal_proxy,
            singleton_metadata.token.clone(),
            internal_proxy_control,
        ) => {
            result?;
        }
        result = tokio::signal::ctrl_c() => {
            result?;
            tracing::info!("server shutdown requested");
        }
        _ = wait_for_external_shutdown(external_shutdown.as_ref()) => {
            tracing::info!("server shutdown requested from external process controller");
        }
        _ = shutdown_signal.cancelled() => {
            tracing::info!("server shutdown requested from singleton control");
        }
    }

    tracing::info!("terminating unified exec processes");
    registry.terminate_unified_exec_processes().await;
    tracing::info!("completing deferred items for active turns");
    runtime.shutdown().await;
    Ok(())
}

fn print_existing_server_status(
    metadata: &crate::singleton::ServerLockMetadata,
    status: &str,
    enabled_protocols: Option<&ProtocolSet>,
) {
    println!("devo server {status}");
    println!("pid: {}", metadata.pid);
    println!("endpoint: {}", metadata.endpoint);
    println!("started_at: {}", metadata.started_at);
    if let Some(enabled_protocols) = enabled_protocols {
        println!("protocols: {enabled_protocols}");
    }
}

/// Merge MCP servers declared in the TUI's global `settings.json`
/// (`mcpServers`) into the runtime MCP config.
///
/// The TUI's `/mcp add` / `/mcp remove` commands write that file and tell the
/// user the server is "available next turn through mcp" (the per-turn registry
/// rebuild re-runs discovery). The server itself only reads
/// `[mcp_servers.<id>]` from config.toml, so without this bridge those
/// declarations never reach the runtime. config.toml wins on id conflicts;
/// malformed entries are skipped with a warning rather than failing boot.
fn merge_tui_settings_mcp_servers(
    mut config: devo_core::McpConfig,
    devo_home: &std::path::Path,
) -> devo_core::McpConfig {
    let settings_path = devo_home.join("settings.json");
    let Ok(raw) = std::fs::read_to_string(&settings_path) else {
        return config;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        tracing::warn!(
            path = %settings_path.display(),
            "skipping TUI settings.json MCP servers: file is not valid JSON"
        );
        return config;
    };
    let Some(servers) = value.get("mcpServers").and_then(|v| v.as_object()) else {
        return config;
    };
    for (name, spec) in servers {
        if config.servers.iter().any(|record| record.id.0 == *name) {
            continue;
        }
        match tui_settings_mcp_server_record(name, spec) {
            Ok(record) => config.servers.push(record),
            Err(error) => tracing::warn!(
                server = %name,
                error = %error,
                "skipping invalid settings.json MCP server entry"
            ),
        }
    }
    config
}

/// Convert one TUI `mcpServers.<name>` entry into an [`McpServerRecord`].
///
/// Shape (apps/tui settings-manager `McpServerConfig`): stdio servers carry
/// `command` + `args` + optional `cwd`/`env` (values are `{ "env": NAME }`
/// references to inherit from the process environment); http servers carry
/// `url` (+ optional headers). `enabled: false` keeps the record listed but
/// disabled so `/mcp remove`/re-add semantics stay reversible.
fn tui_settings_mcp_server_record(
    name: &str,
    spec: &serde_json::Value,
) -> Result<devo_core::McpServerRecord, String> {
    use devo_core::McpServerRecord;
    if !spec.is_object() {
        return Err("entry must be an object".into());
    }
    let enabled = spec
        .get("enabled")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let record = |transport, enabled| McpServerRecord {
        id: devo_core::McpServerId(name.to_string()),
        display_name: name.to_string(),
        transport,
        startup_policy: devo_core::McpStartupPolicy::Eager,
        enabled,
        trust_policy: Default::default(),
        allowed_capabilities: Default::default(),
        roots_policy: Default::default(),
        output_limits: Default::default(),
        auth_ref: None,
    };
    match spec.get("type").and_then(serde_json::Value::as_str) {
        Some("stdio") => {
            let command = spec
                .get("command")
                .and_then(serde_json::Value::as_str)
                .ok_or("stdio server requires a `command` string")?;
            let mut argv = vec![command.to_string()];
            if let Some(args) = spec.get("args").and_then(serde_json::Value::as_array) {
                argv.extend(
                    args.iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_string),
                );
            }
            let cwd = spec
                .get("cwd")
                .and_then(serde_json::Value::as_str)
                .map(std::path::PathBuf::from);
            let mut env_vars = Vec::new();
            if let Some(env) = spec.get("env").and_then(serde_json::Value::as_object) {
                for (child_key, value) in env {
                    // TUI shape: `{ "<child var>": { "env": "<parent var>" } }` —
                    // inherit the parent variable's value. Literal string values
                    // are not representable here; config.toml covers those.
                    if let Some(parent) = value.get("env").and_then(serde_json::Value::as_str) {
                        if parent != child_key {
                            tracing::warn!(
                                server = %name,
                                key = %child_key,
                                parent = %parent,
                                "settings.json env remapping inherits the parent variable name"
                            );
                        }
                        env_vars.push(devo_core::McpServerEnvVar::Name(parent.to_string()));
                    }
                }
            }
            Ok(record(
                devo_core::McpTransportConfig::Stdio {
                    command: argv,
                    cwd,
                    env: Default::default(),
                    env_vars,
                },
                enabled,
            ))
        }
        Some("http") => {
            let url = spec
                .get("url")
                .and_then(serde_json::Value::as_str)
                .ok_or("http server requires a `url` string")?
                .to_string();
            let mut http_headers = std::collections::BTreeMap::new();
            if let Some(headers) = spec.get("headers").and_then(serde_json::Value::as_object) {
                for (key, value) in headers {
                    if let Some(value) = value.as_str() {
                        http_headers.insert(key.clone(), value.to_string());
                    }
                }
            }
            Ok(record(
                devo_core::McpTransportConfig::StreamableHttp {
                    url,
                    auth: None,
                    http_headers,
                    env_http_headers: Default::default(),
                },
                enabled,
            ))
        }
        other => Err(format!(
            "unsupported server type {:?} (expected \"stdio\" or \"http\")",
            other
        )),
    }
}

async fn wait_for_external_shutdown(
    external_shutdown: Option<&tokio_util::sync::CancellationToken>,
) {
    if let Some(token) = external_shutdown {
        token.cancelled().await;
    } else {
        std::future::pending::<()>().await;
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::ServerProcessArgs;
    use super::ServerTransportMode;
    use super::merge_tui_settings_mcp_servers;
    use crate::ProtocolSet;
    use clap::Parser;

    #[test]
    fn tui_settings_mcp_servers_merge_into_runtime_config() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("settings.json"),
            serde_json::json!({
                "mcpServers": {
                    "advtest": {"type": "stdio", "command": "python3", "args": ["/tmp/echo_server.py"]},
                    "remote": {"type": "http", "url": "http://127.0.0.1:9999/mcp"},
                    "broken": {"type": "weird"}
                }
            })
            .to_string(),
        )
        .expect("write settings");

        let mut config = devo_core::McpConfig::default();
        config.servers.push(devo_core::McpServerRecord {
            id: devo_core::McpServerId("advtest".into()),
            display_name: "from toml".into(),
            transport: devo_core::McpTransportConfig::Stdio {
                command: vec!["true".into()],
                cwd: None,
                env: Default::default(),
                env_vars: vec![],
            },
            startup_policy: Default::default(),
            enabled: true,
            trust_policy: Default::default(),
            allowed_capabilities: Default::default(),
            roots_policy: Default::default(),
            output_limits: Default::default(),
            auth_ref: None,
        });

        let merged = merge_tui_settings_mcp_servers(config, dir.path());
        let ids: Vec<&str> = merged
            .servers
            .iter()
            .map(|record| record.id.0.as_str())
            .collect();
        assert!(ids.contains(&"remote"), "http entry merges: {ids:?}");
        assert!(!ids.contains(&"broken"), "invalid entry skipped: {ids:?}");
        // config.toml wins on id conflict: the toml record survives untouched.
        let advtest = merged
            .servers
            .iter()
            .find(|record| record.id.0 == "advtest")
            .expect("advtest kept");
        assert_eq!(advtest.display_name, "from toml");
        let remote = merged
            .servers
            .iter()
            .find(|record| record.id.0 == "remote")
            .expect("remote merged");
        assert!(matches!(
            remote.transport,
            devo_core::McpTransportConfig::StreamableHttp { .. }
        ));
        assert!(remote.enabled);
    }

    #[test]
    fn tui_settings_missing_or_invalid_file_is_noop() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Deliberately empty: no built-in MCP servers.
        let empty = devo_core::McpConfig {
            servers: Vec::new(),
            auto_start: true,
        };
        assert_eq!(
            merge_tui_settings_mcp_servers(empty.clone(), dir.path())
                .servers
                .len(),
            0,
            "missing settings.json is a no-op"
        );
        std::fs::write(dir.path().join("settings.json"), "not json").expect("write invalid");
        assert_eq!(
            merge_tui_settings_mcp_servers(empty, dir.path())
                .servers
                .len(),
            0,
            "invalid settings.json is a no-op"
        );
    }

    #[test]
    fn server_process_args_default_to_config_transport() {
        let args = ServerProcessArgs::parse_from(["devo-server"]);

        assert_eq!(args.transport, ServerTransportMode::Config);
        assert_eq!(args.protocols, ProtocolSet::default());
        assert_eq!(
            args.action().expect("action"),
            super::ServerProcessAction::Run
        );
    }

    #[test]
    fn server_process_args_accept_protocol_sets() {
        let args =
            ServerProcessArgs::parse_from(["devo-server", "--protocols", "native, acp,native"]);

        assert_eq!(args.protocols.names(), vec!["native", "acp"]);
    }

    #[test]
    fn server_process_args_accept_stdio_transport_override() {
        let args = ServerProcessArgs::parse_from(["devo-server", "--transport", "stdio"]);

        assert_eq!(args.transport, ServerTransportMode::Stdio);
        assert_eq!(
            args.action().expect("action"),
            super::ServerProcessAction::Run
        );
    }

    #[test]
    fn server_process_args_accept_websocket_transport_override() {
        let args = ServerProcessArgs::parse_from(["devo-server", "--transport", "websocket"]);

        assert_eq!(args.transport, ServerTransportMode::WebSocket);
        assert_eq!(
            args.action().expect("action"),
            super::ServerProcessAction::Run
        );
    }

    #[test]
    fn server_process_args_accept_status_action() {
        let args = ServerProcessArgs::parse_from(["devo-server", "--status"]);

        assert_eq!(
            args.action().expect("action"),
            super::ServerProcessAction::Status
        );
    }

    #[test]
    fn server_process_args_accept_shutdown_action() {
        let args = ServerProcessArgs::parse_from(["devo-server", "--shutdown"]);

        assert_eq!(
            args.action().expect("action"),
            super::ServerProcessAction::Shutdown
        );
    }

    #[test]
    fn server_process_args_reject_conflicting_actions() {
        let args = ServerProcessArgs::parse_from(["devo-server", "--status", "--shutdown"]);

        assert_eq!(
            args.action().expect_err("conflicting actions").to_string(),
            "--status and --shutdown cannot be used together"
        );
    }

    #[test]
    fn server_process_args_reject_working_root() {
        let error = ServerProcessArgs::try_parse_from(["devo-server", "--working-root", "."])
            .expect_err("working root is no longer a server bootstrap parameter");

        assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
    }
}

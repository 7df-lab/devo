//! Launch a supplier in the bundled web MCP companion, not in the Devo process.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use devo_core::{
    AuthCredentialKind, BuiltinWebSupplier, McpCapability, McpConfig, McpManager, McpOutputLimits,
    McpRootsPolicy, McpServerId, McpServerRecord, McpStartupPolicy, McpTransportConfig,
    McpTrustPolicy, OAuthCredentialsStoreMode, read_user_auth_config,
};
use devo_mcp::manager::RmcpMcpManager;

use crate::session_context::SessionRuntimeContext;

pub(super) const SEARCH_TOOL: &str = "web_search";
pub(super) const SUPPLIERS: [BuiltinWebSupplier; 2] =
    [BuiltinWebSupplier::Tavily, BuiltinWebSupplier::DeepSeek];
const BINARY_NAME: &str = if cfg!(windows) {
    "devo-web-mcp.exe"
} else {
    "devo-web-mcp"
};
const API_KEY_REQUIRED: &str = "API key required";
const BINARY_MISSING: &str = "Built-in MCP executable missing; install or build devo-web-mcp";

pub(super) fn server_id(supplier: BuiltinWebSupplier) -> &'static str {
    match supplier {
        BuiltinWebSupplier::Tavily => "devo:builtin:tavily",
        BuiltinWebSupplier::DeepSeek => "devo:builtin:deepseek",
    }
}

pub(super) fn supplier_for_server_id(id: &str) -> Option<BuiltinWebSupplier> {
    SUPPLIERS
        .into_iter()
        .find(|supplier| server_id(*supplier) == id)
}

pub(super) fn label(supplier: BuiltinWebSupplier) -> &'static str {
    match supplier {
        BuiltinWebSupplier::Tavily => "Tavily · Built-in MCP",
        BuiltinWebSupplier::DeepSeek => "DeepSeek · Built-in MCP",
    }
}

fn credential_id(supplier: BuiltinWebSupplier) -> &'static str {
    match supplier {
        BuiltinWebSupplier::Tavily => "tavily",
        BuiltinWebSupplier::DeepSeek => "deepseek",
    }
}

pub(super) fn status_note(
    context: &SessionRuntimeContext,
    supplier: BuiltinWebSupplier,
) -> Option<String> {
    let result = context
        .config_store
        .lock()
        .map_err(|_| anyhow::anyhow!(API_KEY_REQUIRED))
        .and_then(|store| check_key(store.user_config_dir(), supplier))
        .and_then(|()| binary_path());
    result.err().map(|error| error.to_string())
}

fn binary_path() -> anyhow::Result<PathBuf> {
    // Never execute binaries found via PATH or the current workspace.
    std::env::current_exe()
        .ok()
        .map(|path| path.with_file_name(BINARY_NAME))
        .filter(|path| path.is_file())
        .ok_or_else(|| anyhow::anyhow!(BINARY_MISSING))
}

/// Construct a lazy stdio connection. Only the config paths and supplier name
/// cross the process boundary; no credentials are passed in argv or environment.
pub(super) fn manager(
    context: &SessionRuntimeContext,
    supplier: BuiltinWebSupplier,
) -> anyhow::Result<Arc<dyn McpManager>> {
    let (config_dir, workspace_root) = {
        let store = context
            .config_store
            .lock()
            .map_err(|_| anyhow::anyhow!(API_KEY_REQUIRED))?;
        (
            store.user_config_dir().to_owned(),
            store.workspace_root().map(Path::to_path_buf),
        )
    };
    check_key(&config_dir, supplier)?;
    let path = binary_path()?;
    let path_string = |path: &Path| {
        path.to_str()
            .map(str::to_owned)
            .ok_or_else(|| anyhow::anyhow!("Cannot launch the built-in MCP with a non-UTF-8 path"))
    };
    let mut command = vec![
        path_string(&path)?,
        credential_id(supplier).into(),
        "--config-dir".into(),
        path_string(&config_dir)?,
    ];
    if let Some(workspace_root) = workspace_root.as_deref() {
        command.push("--workspace-root".into());
        command.push(path_string(workspace_root)?);
    }
    let config = McpConfig {
        servers: vec![McpServerRecord {
            id: McpServerId(server_id(supplier).into()),
            display_name: label(supplier).into(),
            transport: McpTransportConfig::Stdio {
                command,
                cwd: path.parent().map(Path::to_path_buf),
                env: BTreeMap::new(),
                env_vars: Vec::new(),
            },
            startup_policy: McpStartupPolicy::Lazy,
            enabled: true,
            trust_policy: McpTrustPolicy::User,
            allowed_capabilities: vec![McpCapability::Tools],
            roots_policy: McpRootsPolicy::None,
            output_limits: McpOutputLimits::default(),
            auth_ref: None,
        }],
        auto_start: false,
    };
    Ok(Arc::new(RmcpMcpManager::new(
        config,
        OAuthCredentialsStoreMode::default(),
    )))
}

fn check_key(config_dir: &Path, supplier: BuiltinWebSupplier) -> anyhow::Result<()> {
    let auth = read_user_auth_config(&config_dir.join(devo_core::AUTH_CONFIG_FILE_NAME))
        .map_err(|_| anyhow::anyhow!(API_KEY_REQUIRED))?;
    if !auth
        .credentials
        .get(credential_id(supplier))
        .is_some_and(|credential| {
            credential.kind == AuthCredentialKind::ApiKey && !credential.value.trim().is_empty()
        })
    {
        anyhow::bail!(API_KEY_REQUIRED);
    }
    Ok(())
}

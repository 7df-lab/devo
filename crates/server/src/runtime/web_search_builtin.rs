//! Launch a supplier in the bundled web MCP companion, not in the Devo process.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use async_trait::async_trait;
use devo_protocol::native::rpc_session::WebApiKeyStatus;
use serde_json::Value;
use sha2::{Digest, Sha256};

use devo_core::{
    AuthCredentialKind, BuiltinWebSupplier, McpCapability, McpConfig, McpManager, McpOutputLimits,
    McpRootsPolicy, McpServerId, McpServerRecord, McpStartupPolicy, McpTransportConfig,
    McpTrustPolicy, OAuthCredentialsStoreMode, read_user_auth_config,
};
use devo_mcp::manager::RmcpMcpManager;

use crate::session_context::SessionRuntimeContext;

pub(super) const SEARCH_TOOL: &str = "web_search";
pub(super) const SUPPLIERS: [BuiltinWebSupplier; 4] = [
    BuiltinWebSupplier::Tavily,
    BuiltinWebSupplier::DeepSeek,
    BuiltinWebSupplier::Serper,
    BuiltinWebSupplier::Exa,
];
const BINARY_NAME: &str = if cfg!(windows) {
    "devo-web-mcp.exe"
} else {
    "devo-web-mcp"
};
const API_KEY_REQUIRED: &str = "API key required";
const BINARY_MISSING: &str = "Built-in MCP executable missing; install or build devo-web-mcp";
const MAX_REJECTION_EVIDENCE: usize = 128;
const AUTH_REJECTED_CODE: &str = "devo_credential_rejected";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct EvidenceKey {
    supplier: &'static str,
    key_fingerprint: [u8; 32],
}

#[derive(Default)]
struct RejectionEvidence {
    rejected: HashMap<EvidenceKey, ()>,
    order: VecDeque<EvidenceKey>,
}

static REJECTIONS: LazyLock<Mutex<RejectionEvidence>> =
    LazyLock::new(|| Mutex::new(RejectionEvidence::default()));

pub(super) fn server_id(supplier: BuiltinWebSupplier) -> &'static str {
    match supplier {
        BuiltinWebSupplier::Tavily => "devo:builtin:tavily",
        BuiltinWebSupplier::DeepSeek => "devo:builtin:deepseek",
        BuiltinWebSupplier::Serper => "devo:builtin:serper",
        BuiltinWebSupplier::Exa => "devo:builtin:exa",
    }
}

pub(super) fn supplier_for_server_id(id: &str) -> Option<BuiltinWebSupplier> {
    SUPPLIERS
        .into_iter()
        .find(|supplier| server_id(*supplier) == id)
}

pub(super) fn label(supplier: BuiltinWebSupplier) -> &'static str {
    match supplier {
        BuiltinWebSupplier::Tavily => "Tavily · Built-in",
        BuiltinWebSupplier::DeepSeek => "DeepSeek · Built-in",
        BuiltinWebSupplier::Serper => "Serper · Built-in",
        BuiltinWebSupplier::Exa => "Exa · Built-in",
    }
}

fn credential_id(supplier: BuiltinWebSupplier) -> &'static str {
    match supplier {
        BuiltinWebSupplier::Tavily => "tavily",
        BuiltinWebSupplier::DeepSeek => "deepseek",
        BuiltinWebSupplier::Serper => "serper",
        BuiltinWebSupplier::Exa => "exa",
    }
}

pub(super) fn credential_status(
    context: &SessionRuntimeContext,
    supplier: BuiltinWebSupplier,
) -> (WebApiKeyStatus, Option<String>) {
    let key_fingerprint = context.config_store.lock().ok().and_then(|store| {
        credential_fingerprint(store.user_config_dir(), supplier)
            .ok()
            .flatten()
    });
    let Some(key_fingerprint) = key_fingerprint else {
        return (WebApiKeyStatus::Required, None);
    };
    let key = EvidenceKey {
        supplier: credential_id(supplier),
        key_fingerprint,
    };
    let rejected = REJECTIONS
        .lock()
        .ok()
        .is_some_and(|evidence| evidence.rejected.contains_key(&key));
    let note = binary_path().err().map(|error| error.to_string());
    (
        if rejected {
            WebApiKeyStatus::Rejected
        } else {
            WebApiKeyStatus::Saved
        },
        note,
    )
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
    let inner: Arc<dyn McpManager> = Arc::new(RmcpMcpManager::new(
        config,
        OAuthCredentialsStoreMode::default(),
    ));
    let key_fingerprint = credential_fingerprint(&config_dir, supplier)?
        .ok_or_else(|| anyhow::anyhow!(API_KEY_REQUIRED))?;
    Ok(Arc::new(EvidenceManager {
        inner,
        config_dir,
        supplier,
        evidence_key: EvidenceKey {
            supplier: credential_id(supplier),
            key_fingerprint,
        },
    }))
}

fn credential_fingerprint(
    config_dir: &Path,
    supplier: BuiltinWebSupplier,
) -> anyhow::Result<Option<[u8; 32]>> {
    let auth = read_user_auth_config(&config_dir.join(devo_core::AUTH_CONFIG_FILE_NAME))
        .map_err(|_| anyhow::anyhow!(API_KEY_REQUIRED))?;
    let Some(credential) = auth.credentials.get(credential_id(supplier)) else {
        return Ok(None);
    };
    if credential.kind != AuthCredentialKind::ApiKey || credential.value.trim().is_empty() {
        return Ok(None);
    }
    let digest = Sha256::digest(credential.value.trim().as_bytes());
    let mut fingerprint = [0; 32];
    fingerprint.copy_from_slice(&digest);
    Ok(Some(fingerprint))
}

fn check_key(config_dir: &Path, supplier: BuiltinWebSupplier) -> anyhow::Result<()> {
    credential_fingerprint(config_dir, supplier)?
        .map(|_| ())
        .ok_or_else(|| anyhow::anyhow!(API_KEY_REQUIRED))
}

struct EvidenceManager {
    inner: Arc<dyn McpManager>,
    config_dir: PathBuf,
    supplier: BuiltinWebSupplier,
    evidence_key: EvidenceKey,
}

#[async_trait]
impl McpManager for EvidenceManager {
    async fn statuses(&self) -> Result<Vec<devo_core::McpServerStatus>, devo_core::McpError> {
        self.inner.statuses().await
    }

    async fn discover_tools(&self) -> Result<Vec<devo_core::McpToolInfo>, devo_core::McpError> {
        self.inner.discover_tools().await
    }

    async fn refresh(
        &self,
        server_id: &McpServerId,
    ) -> Result<devo_core::McpServerStatus, devo_core::McpError> {
        self.inner.refresh(server_id).await
    }

    async fn reconnect(
        &self,
        server_id: &McpServerId,
    ) -> Result<devo_core::McpServerStatus, devo_core::McpError> {
        self.inner.reconnect(server_id).await
    }

    async fn cached_tools(
        &self,
        server_id: &McpServerId,
    ) -> Result<Vec<devo_core::McpToolInfo>, devo_core::McpError> {
        self.inner.cached_tools(server_id).await
    }

    async fn configure_enabled(
        &self,
        server_id: &McpServerId,
        enabled: bool,
    ) -> Result<devo_core::McpServerStatus, devo_core::McpError> {
        self.inner.configure_enabled(server_id, enabled).await
    }

    async fn set_enabled(
        &self,
        server_id: &McpServerId,
        enabled: bool,
    ) -> Result<devo_core::McpServerStatus, devo_core::McpError> {
        self.inner.set_enabled(server_id, enabled).await
    }

    async fn invoke_tool(
        &self,
        server_id: &McpServerId,
        tool_name: &str,
        input: Value,
    ) -> Result<Value, devo_core::McpError> {
        let result = self.inner.invoke_tool(server_id, tool_name, input).await?;
        let credential_is_current = credential_fingerprint(&self.config_dir, self.supplier)
            .ok()
            .flatten()
            == Some(self.evidence_key.key_fingerprint);
        if credential_is_current
            && result.get("isError").and_then(Value::as_bool) == Some(true)
            && result
                .pointer("/structuredContent/_devo/code")
                .and_then(Value::as_str)
                == Some(AUTH_REJECTED_CODE)
        {
            record_rejection(self.evidence_key);
        } else if credential_is_current
            && result.get("isError").and_then(Value::as_bool) != Some(true)
        {
            clear_rejection(self.evidence_key);
        }
        Ok(result)
    }

    async fn read_resource(
        &self,
        server_id: &McpServerId,
        uri: &str,
    ) -> Result<Value, devo_core::McpError> {
        self.inner.read_resource(server_id, uri).await
    }
}

fn record_rejection(key: EvidenceKey) {
    let Ok(mut evidence) = REJECTIONS.lock() else {
        return;
    };
    if evidence.rejected.insert(key, ()).is_none() {
        evidence.order.push_back(key);
    }
    while evidence.order.len() > MAX_REJECTION_EVIDENCE {
        if let Some(oldest) = evidence.order.pop_front() {
            evidence.rejected.remove(&oldest);
        }
    }
}

fn clear_rejection(key: EvidenceKey) {
    let Ok(mut evidence) = REJECTIONS.lock() else {
        return;
    };
    evidence.rejected.remove(&key);
    evidence.order.retain(|entry| *entry != key);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejection_evidence_is_generation_scoped_and_bounded() {
        let first = EvidenceKey {
            supplier: "evidence-test",
            key_fingerprint: [1; 32],
        };
        let replacement = EvidenceKey {
            supplier: "evidence-test",
            key_fingerprint: [2; 32],
        };
        record_rejection(first);
        assert!(REJECTIONS.lock().unwrap().rejected.contains_key(&first));
        assert!(
            !REJECTIONS
                .lock()
                .unwrap()
                .rejected
                .contains_key(&replacement)
        );
        clear_rejection(first);
        assert!(!REJECTIONS.lock().unwrap().rejected.contains_key(&first));

        let keys: Vec<_> = (0..=MAX_REJECTION_EVIDENCE)
            .map(|index| EvidenceKey {
                supplier: "bounded-evidence-test",
                key_fingerprint: [index as u8; 32],
            })
            .collect();
        for key in keys.iter().copied() {
            record_rejection(key);
        }
        let evidence = REJECTIONS.lock().unwrap();
        assert!(evidence.order.len() <= MAX_REJECTION_EVIDENCE);
        assert!(!evidence.rejected.contains_key(&keys[0]));
        assert!(evidence.rejected.contains_key(keys.last().unwrap()));
        drop(evidence);
        for key in keys {
            clear_rejection(key);
        }
    }
}

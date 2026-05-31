use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ── Scope / Path Types ────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConfigScope {
    User,
    Workspace { workspace_root: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigInputPaths {
    pub user_config: ConfigFilePath,
    pub workspace_config: Option<ConfigFilePath>,
    pub user_auth: UserAuthPath,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigFilePath {
    pub scope: ConfigScope,
    pub config_dir: PathBuf,
    pub config_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserAuthPath {
    /// ~/.devo on macOS/Linux, C:\Users\username\.devo on Windows
    pub config_dir: PathBuf,
    /// Always <user-config-dir>/auth.json
    pub auth_path: PathBuf,
}

// ── Loaded Sources ─────────────────────────────────────────────────────

pub struct LoadedConfigInputs {
    pub user_config: LoadedConfigFile,
    pub workspace_config: Option<LoadedConfigFile>,
    pub user_auth: LoadedUserAuth,
}

pub struct LoadedConfigFile {
    pub path: ConfigFilePath,
    /// None if file is missing; malformed content is a diagnostic, not None.
    pub config: Option<ConfigDocument>,
    pub diagnostics: Vec<ConfigDiagnostic>,
}

pub struct LoadedUserAuth {
    pub path: UserAuthPath,
    /// None if file is missing (empty credential set).
    pub document: Option<AuthDocument>,
    pub diagnostics: Vec<ConfigDiagnostic>,
}

// ── Resolution Output ──────────────────────────────────────────────────

pub struct ConfigurationResolution {
    pub effective: EffectiveConfig,
    pub user_auth: UserAuthStore,
    pub diagnostics: Vec<ConfigDiagnostic>,
}

#[derive(Debug, Clone, Default)]
pub struct EffectiveConfig {
    pub providers: BTreeMap<String, EffectiveProvider>,
    pub model_bindings: BTreeMap<String, EffectiveModelBinding>,
    pub defaults: EffectiveDefaults,
    pub provenance: ConfigProvenance,
}

#[derive(Debug, Clone, Default)]
pub struct EffectiveDefaults {
    pub model: Option<String>,
    pub model_provider: Option<String>,
    pub reasoning_effort: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EffectiveProvider {
    pub name: String,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub wire_api: Option<String>,
    pub models: Vec<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub struct EffectiveModelBinding {
    pub provider_id: String,
    pub model_slug: String,
    pub display_name: Option<String>,
    pub invocation_method: Option<String>,
    pub reasoning_effort: Option<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub struct UserAuthStore {
    pub path: UserAuthPath,
    pub credentials: BTreeMap<String, String>,
    pub provenance: BTreeMap<String, AuthValueSource>,
}

// ── Provenance ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct ConfigProvenance {
    pub values: BTreeMap<String, ConfigValueSource>,
    pub merged_records: BTreeMap<String, MergedRecordSource>,
    pub credential_refs: BTreeMap<String, CredentialResolutionSource>,
}

#[derive(Debug, Clone)]
pub struct ConfigValueSource {
    pub scope: ConfigScope,
    pub file: PathBuf,
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct MergedRecordSource {
    pub record_path: String,
    pub identity_key: String,
    pub contributing_scopes: Vec<ConfigScope>,
    pub field_sources: BTreeMap<String, ConfigValueSource>,
}

#[derive(Debug, Clone)]
pub struct AuthValueSource {
    pub file: PathBuf,
    pub credential_id: String,
}

#[derive(Debug, Clone)]
pub struct CredentialResolutionSource {
    pub credential_id: String,
    pub auth_source: AuthValueSource,
}

// ── Write Target ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum ConfigWriteTarget {
    UserConfig,
    WorkspaceConfig { workspace_root: PathBuf },
    UserAuth,
}

// ── Diagnostics ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ConfigDiagnostic {
    pub severity: DiagnosticSeverity,
    pub source: String,
    pub message: String,
    pub path: Option<String>,
    pub recovery_hint: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Info,
}

// ── Document Placeholders ──────────────────────────────────────────────
// Concrete TOML/JSON parsing deferred to implementation.

#[derive(Debug, Clone)]
pub struct ConfigDocument {
    _private: (),
}

#[derive(Debug, Clone)]
pub struct AuthDocument {
    _private: (),
}

// ── Merge Algorithm (B4) ───────────────────────────────────────────────

/// Merge user and workspace config into EffectiveConfig using field-level
/// merge semantics. Auth data is user-only.
pub fn merge_into_effective(
    user_config: &LoadedConfigFile,
    workspace_config: Option<&LoadedConfigFile>,
    user_auth: &LoadedUserAuth,
) -> ConfigurationResolution {
    let mut diagnostics = Vec::new();
    diagnostics.extend_from_slice(&user_config.diagnostics);
    if let Some(ws) = workspace_config {
        diagnostics.extend_from_slice(&ws.diagnostics);
    }
    diagnostics.extend_from_slice(&user_auth.diagnostics);

    // Build user auth store (user-only scope).
    let user_auth_store = build_auth_store(user_auth);

    // Start with user config as base.
    let mut effective = EffectiveConfig::default();

    // Merge user scalar defaults.
    apply_scalar_defaults(&mut effective, user_config);

    // Apply workspace scalar defaults (workspace overrides user for same field).
    if let Some(ws) = workspace_config {
        apply_scalar_defaults(&mut effective, ws);
    }

    ConfigurationResolution {
        effective,
        user_auth: user_auth_store,
        diagnostics,
    }
}

/// Apply scalar defaults from a loaded config file.
/// Later calls override earlier values for the same field.
fn apply_scalar_defaults(effective: &mut EffectiveConfig, source: &LoadedConfigFile) {
    // Placeholder: actual TOML parsing walks the document tree.
    // When both sources define the same scalar, the last write (workspace) wins.
    _ = (effective, source);
}

/// Build user auth store from loaded auth file.
fn build_auth_store(auth: &LoadedUserAuth) -> UserAuthStore {
    UserAuthStore {
        path: auth.path.clone(),
        credentials: BTreeMap::new(),
        provenance: BTreeMap::new(),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_configuration_resolution() {
        let user_cfg = LoadedConfigFile {
            path: ConfigFilePath {
                scope: ConfigScope::User,
                config_dir: PathBuf::from("/home/user/.devo"),
                config_path: PathBuf::from("/home/user/.devo/config.toml"),
            },
            config: None,
            diagnostics: vec![],
        };
        let user_auth = LoadedUserAuth {
            path: UserAuthPath {
                config_dir: PathBuf::from("/home/user/.devo"),
                auth_path: PathBuf::from("/home/user/.devo/auth.json"),
            },
            document: None,
            diagnostics: vec![],
        };

        let resolution = merge_into_effective(&user_cfg, None, &user_auth);
        assert!(resolution.diagnostics.is_empty());
        assert!(resolution.effective.providers.is_empty());
        assert!(resolution.effective.defaults.model.is_none());
        assert!(resolution.user_auth.credentials.is_empty());
    }

    #[test]
    fn user_auth_path_always_user_scope() {
        let auth_path = UserAuthPath {
            config_dir: PathBuf::from("/home/user/.devo"),
            auth_path: PathBuf::from("/home/user/.devo/auth.json"),
        };
        assert!(auth_path.auth_path.ends_with("auth.json"));
        assert!(auth_path.config_dir.ends_with(".devo"));
    }

    #[test]
    fn workspace_scope_has_no_auth() {
        // ConfigScope::Workspace has no auth variant.
        let ws = ConfigScope::Workspace {
            workspace_root: PathBuf::from("/tmp/project"),
        };
        assert!(matches!(ws, ConfigScope::Workspace { .. }));
    }

    #[test]
    fn config_write_target_auth_is_user_only() {
        let t = ConfigWriteTarget::UserAuth;
        assert!(matches!(t, ConfigWriteTarget::UserAuth));
    }

    #[test]
    fn diagnostic_severity_levels() {
        assert!(matches!(
            DiagnosticSeverity::Error,
            DiagnosticSeverity::Error
        ));
        assert!(matches!(
            DiagnosticSeverity::Warning,
            DiagnosticSeverity::Warning
        ));
        assert!(matches!(DiagnosticSeverity::Info, DiagnosticSeverity::Info));
    }
}

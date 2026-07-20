use std::path::Path;
use std::str::FromStr;

use devo_util_paths::absolute_path::AbsolutePathBuf;
use serde::{Deserialize, Serialize};
use strum_macros::Display;
use tracing::error;

use super::permissions::{
    FileSystemAccessMode, FileSystemPath, FileSystemSandboxEntry, FileSystemSandboxPolicy,
    FileSystemSpecialPath, NetworkSandboxPolicy, default_read_only_subpaths_for_writable_root,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, Default)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum NetworkAccess {
    #[default]
    Restricted,
    Enabled,
}

impl NetworkAccess {
    pub fn is_enabled(self) -> bool {
        matches!(self, NetworkAccess::Enabled)
    }
}

/// Legacy sandbox policy shape retained for permission-profile conversion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Display)]
#[strum(serialize_all = "kebab-case")]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum SandboxPolicy {
    #[serde(rename = "danger-full-access")]
    DangerFullAccess,
    #[serde(rename = "read-only")]
    ReadOnly {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        network_access: bool,
    },
    #[serde(rename = "external-sandbox")]
    ExternalSandbox {
        #[serde(default)]
        network_access: NetworkAccess,
    },
    #[serde(rename = "workspace-write")]
    WorkspaceWrite {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        writable_roots: Vec<AbsolutePathBuf>,
        #[serde(default)]
        network_access: bool,
        #[serde(default)]
        exclude_tmpdir_env_var: bool,
        #[serde(default)]
        exclude_slash_tmp: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WritableRoot {
    pub root: AbsolutePathBuf,
    pub read_only_subpaths: Vec<AbsolutePathBuf>,
    pub protected_metadata_names: Vec<String>,
}

impl WritableRoot {
    pub fn is_path_writable(&self, path: &Path) -> bool {
        if !path.starts_with(&self.root) {
            return false;
        }
        for subpath in &self.read_only_subpaths {
            if path.starts_with(subpath) {
                return false;
            }
        }
        if self.path_contains_protected_metadata_name(path) {
            return false;
        }
        true
    }

    fn path_contains_protected_metadata_name(&self, path: &Path) -> bool {
        let Ok(relative_path) = path.strip_prefix(&self.root) else {
            return false;
        };
        let Some(first_component) = relative_path.components().next() else {
            return false;
        };
        self.protected_metadata_names
            .iter()
            .any(|name| first_component.as_os_str() == std::ffi::OsStr::new(name))
    }
}

impl FromStr for SandboxPolicy {
    type Err = serde_json::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s)
    }
}

impl SandboxPolicy {
    pub fn new_read_only_policy() -> Self {
        SandboxPolicy::ReadOnly {
            network_access: false,
        }
    }

    pub fn new_workspace_write_policy() -> Self {
        SandboxPolicy::WorkspaceWrite {
            writable_roots: Vec::new(),
            network_access: false,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        }
    }

    pub fn has_full_network_access(&self) -> bool {
        match self {
            SandboxPolicy::DangerFullAccess => true,
            SandboxPolicy::ExternalSandbox { network_access } => network_access.is_enabled(),
            SandboxPolicy::ReadOnly { network_access, .. } => *network_access,
            SandboxPolicy::WorkspaceWrite { network_access, .. } => *network_access,
        }
    }

    pub fn get_writable_roots_with_cwd(&self, cwd: &Path) -> Vec<WritableRoot> {
        match self {
            SandboxPolicy::DangerFullAccess | SandboxPolicy::ExternalSandbox { .. } => Vec::new(),
            SandboxPolicy::ReadOnly { .. } => Vec::new(),
            SandboxPolicy::WorkspaceWrite {
                writable_roots,
                exclude_tmpdir_env_var,
                exclude_slash_tmp,
                ..
            } => {
                let mut roots: Vec<AbsolutePathBuf> = writable_roots.clone();
                if let Ok(cwd) = AbsolutePathBuf::from_absolute_path(cwd) {
                    roots.push(cwd);
                } else {
                    error!("Ignoring invalid cwd {cwd:?} for sandbox writable root");
                }
                if cfg!(unix)
                    && !exclude_slash_tmp
                    && let Ok(slash_tmp) = AbsolutePathBuf::from_absolute_path("/tmp")
                    && slash_tmp.as_path().is_dir()
                {
                    roots.push(slash_tmp);
                }
                if !exclude_tmpdir_env_var
                    && let Some(tmpdir) = std::env::var_os("TMPDIR")
                    && !tmpdir.is_empty()
                    && let Ok(tmpdir_path) =
                        AbsolutePathBuf::from_absolute_path(PathBuf::from(&tmpdir))
                {
                    roots.push(tmpdir_path);
                }
                let cwd_root = AbsolutePathBuf::from_absolute_path(cwd).ok();
                roots
                    .into_iter()
                    .map(|writable_root| {
                        let protect_missing_legacy_agent_dir = cwd_root
                            .as_ref()
                            .is_some_and(|cwd_root| cwd_root == &writable_root);
                        WritableRoot {
                            read_only_subpaths: default_read_only_subpaths_for_writable_root(
                                &writable_root,
                                protect_missing_legacy_agent_dir,
                            ),
                            protected_metadata_names: Vec::new(),
                            root: writable_root,
                        }
                    })
                    .collect()
            }
        }
    }
}

impl From<&SandboxPolicy> for NetworkSandboxPolicy {
    fn from(value: &SandboxPolicy) -> Self {
        if value.has_full_network_access() {
            NetworkSandboxPolicy::Enabled
        } else {
            NetworkSandboxPolicy::Restricted
        }
    }
}

impl From<&SandboxPolicy> for FileSystemSandboxPolicy {
    fn from(value: &SandboxPolicy) -> Self {
        match value {
            SandboxPolicy::DangerFullAccess => FileSystemSandboxPolicy::unrestricted(),
            SandboxPolicy::ExternalSandbox { .. } => FileSystemSandboxPolicy::external_sandbox(),
            SandboxPolicy::ReadOnly { .. } => {
                FileSystemSandboxPolicy::restricted(vec![FileSystemSandboxEntry {
                    path: FileSystemPath::Special {
                        value: FileSystemSpecialPath::Root,
                    },
                    access: FileSystemAccessMode::Read,
                }])
            }
            SandboxPolicy::WorkspaceWrite {
                writable_roots,
                exclude_tmpdir_env_var,
                exclude_slash_tmp,
                ..
            } => FileSystemSandboxPolicy::workspace_write(
                writable_roots,
                *exclude_tmpdir_env_var,
                *exclude_slash_tmp,
            ),
        }
    }
}

use std::path::PathBuf;

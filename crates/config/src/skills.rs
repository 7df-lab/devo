use std::path::PathBuf;

use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

/// Stores runtime configuration for skill discovery and change tracking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillsConfig {
    /// Whether skill discovery is enabled at all.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// User-level roots scanned for skills.
    #[serde(default)]
    pub user_roots: Vec<PathBuf>,
    /// Workspace-level roots scanned for skills.
    #[serde(default)]
    pub workspace_roots: Vec<PathBuf>,
    /// Whether the runtime should watch skill roots for changes.
    #[serde(default = "default_true")]
    pub watch_for_changes: bool,
    /// Whether bundled system skills are installed and included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundled: Option<BundledSkillsConfig>,
    /// Whether turns receive automatic available-skills instructions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_instructions: Option<bool>,
    /// Path/name enablement overrides.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub config: Vec<SkillConfig>,
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            user_roots: vec![PathBuf::from("skills")],
            workspace_roots: vec![PathBuf::from("skills")],
            watch_for_changes: true,
            bundled: Some(BundledSkillsConfig::default()),
            include_instructions: Some(true),
            config: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundledSkillsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for BundledSkillsConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

//! Skill package data model per L3-BEH-SKILLS-001 §2-5.

use serde::{Deserialize, Serialize};

/// Stable local identity for a skill package.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SkillPackageId {
    pub source_kind: SkillSourceKind,
    pub package_name: SkillName,
    pub root: String,
}

impl SkillPackageId {
    pub fn new(source_kind: SkillSourceKind, package_name: SkillName, root: String) -> Self {
        Self {
            source_kind,
            package_name,
            root,
        }
    }
}

/// Normalized skill name (lowercase ASCII, digits, _, -, 1-64 chars).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SkillName(pub String);

impl SkillName {
    pub fn new(raw: &str) -> Result<Self, String> {
        let normalized = raw.trim().to_lowercase();
        if normalized.is_empty() || normalized.len() > 64 {
            return Err(format!(
                "name length must be 1-64, got {}",
                normalized.len()
            ));
        }
        if !normalized
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        {
            return Err(format!("invalid characters in name: '{}'", raw));
        }
        Ok(Self(normalized))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SkillName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A fully validated skill package.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillPackage {
    pub package_id: SkillPackageId,
    pub root: String,
    pub definition: SkillDefinition,
    pub resources: Vec<SkillResourceRef>,
    pub content_hash: Sha256Digest,
    pub diagnostics: Vec<SkillDiagnostic>,
}

/// Parsed skill definition from SKILL.md.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillDefinition {
    pub name: SkillName,
    pub description: String,
    pub version: Option<String>,
    pub enabled: Option<bool>,
    pub tags: Vec<String>,
    pub compatibility: Option<SkillCompatibility>,
    pub allowed_tools: Vec<String>,
    pub instruction_body: String,
    pub frontmatter_format: FrontmatterFormat,
}

/// Compatibility constraints for a skill.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillCompatibility {
    pub min_version: Option<String>,
    pub requires_features: Vec<String>,
}

/// Source kind for a skill package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillSourceKind {
    BuiltInDefault,
    User,
    Workspace,
    Plugin,
    ExternalPackage,
}

/// Reference to a supporting resource file within the package.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillResourceRef {
    pub relative_path: String,
    pub resource_kind: SkillResourceKind,
    pub byte_len: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillResourceKind {
    Reference,
    Script,
    Asset,
    Template,
}

/// Frontmatter format identified during parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrontmatterFormat {
    Yaml,
    Toml,
    Unknown,
}

/// SHA-256 digest as a hex string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sha256Digest(pub String);

/// Diagnostic produced during package validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SkillDiagnostic {
    MissingEntrypoint,
    UnreadableEntrypoint { reason: String },
    InvalidFrontmatter { reason: String },
    MissingRequiredField { field: String },
    InvalidName { value: String },
    DescriptionTooLong { bytes: usize },
    BodyTooLarge { bytes: usize },
    ResourcePathEscapesPackage { path: String },
    UnsupportedResourceType { path: String },
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_skill_name() {
        let name = SkillName::new("my-skill_1").expect("valid");
        assert_eq!(name.as_str(), "my-skill_1");

        let name = SkillName::new("Code-Review").expect("valid");
        assert_eq!(name.as_str(), "code-review"); // normalized to lowercase
    }

    #[test]
    fn invalid_skill_name_too_long() {
        let long = "a".repeat(65);
        assert!(SkillName::new(&long).is_err());
    }

    #[test]
    fn invalid_skill_name_empty() {
        assert!(SkillName::new("").is_err());
    }

    #[test]
    fn invalid_skill_name_special_chars() {
        assert!(SkillName::new("my skill").is_err());
        assert!(SkillName::new("skill!").is_err());
    }

    #[test]
    fn skill_package_id_equality() {
        let name = SkillName::new("test").unwrap();
        let id1 = SkillPackageId::new(SkillSourceKind::User, name.clone(), "/root".into());
        let id2 = SkillPackageId::new(SkillSourceKind::User, name, "/root".into());
        assert_eq!(id1, id2);
    }

    #[test]
    fn skill_source_kind_serde_roundtrip() {
        for kind in &[
            SkillSourceKind::BuiltInDefault,
            SkillSourceKind::User,
            SkillSourceKind::Workspace,
            SkillSourceKind::Plugin,
            SkillSourceKind::ExternalPackage,
        ] {
            let json = serde_json::to_string(kind).expect("serialize");
            let restored: SkillSourceKind = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored, *kind);
        }
    }

    #[test]
    fn skill_diagnostic_serde_roundtrip() {
        let diags = vec![
            SkillDiagnostic::MissingEntrypoint,
            SkillDiagnostic::InvalidFrontmatter {
                reason: "bad YAML".into(),
            },
            SkillDiagnostic::MissingRequiredField {
                field: "name".into(),
            },
            SkillDiagnostic::InvalidName {
                value: "bad name".into(),
            },
            SkillDiagnostic::DescriptionTooLong { bytes: 5000 },
            SkillDiagnostic::BodyTooLarge { bytes: 100000 },
            SkillDiagnostic::ResourcePathEscapesPackage {
                path: "../escape".into(),
            },
        ];
        for diag in &diags {
            let json = serde_json::to_string(diag).expect("serialize");
            let restored: SkillDiagnostic = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored, *diag);
        }
    }

    #[test]
    fn frontmatter_format_serde_roundtrip() {
        for fmt in &[
            FrontmatterFormat::Yaml,
            FrontmatterFormat::Toml,
            FrontmatterFormat::Unknown,
        ] {
            let json = serde_json::to_string(fmt).expect("serialize");
            let restored: FrontmatterFormat = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored, *fmt);
        }
    }
}

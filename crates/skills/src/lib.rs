//! Skills crate — package definitions, SKILL.md parsing, and default-skill installation.
//!
//! Implements L3-BEH-SKILLS-001. Owns package mechanics only:
//! type definitions, frontmatter/body parsing, validation, and idempotent
//! default-skill installation. Runtime policy lives in core/server.

pub mod installer;
pub mod package;
pub mod parser;

pub use installer::{
    DefaultSkillInstallFailure, DefaultSkillInstallMode, DefaultSkillInstallOptions,
    DefaultSkillInstallReport, DefaultSkillInstaller,
};
pub use package::{
    FrontmatterFormat, Sha256Digest, SkillCompatibility, SkillDefinition, SkillDiagnostic,
    SkillName, SkillPackage, SkillPackageId, SkillResourceKind, SkillResourceRef, SkillSourceKind,
};
pub use parser::parse_skill_md;

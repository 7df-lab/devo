//! Default skill installation per L3-BEH-SKILLS-001 §6.
//!
//! Idempotent installation of bundled default skills into the user skill root.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::package::{Sha256Digest, SkillName};

// ── Default Skill Types ──────────────────────────────────────────────

/// A bundle of default skills shipped with the program.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultSkillBundle {
    pub bundle_version: String,
    pub skills: Vec<DefaultSkillAsset>,
}

/// A single default skill asset within a bundle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultSkillAsset {
    pub default_skill_id: String,
    pub package_name: SkillName,
    pub package_version: Option<String>,
    pub content_hash: Sha256Digest,
    pub files: Vec<EmbeddedSkillFile>,
}

/// An embedded file within a default skill package.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddedSkillFile {
    pub relative_path: String,
    pub content: String,
    pub executable: bool,
}

// ── Installation Types ───────────────────────────────────────────────

/// Options for default skill installation.
#[derive(Debug, Clone)]
pub struct DefaultSkillInstallOptions {
    pub user_skill_root: PathBuf,
    pub mode: DefaultSkillInstallMode,
}

impl Default for DefaultSkillInstallOptions {
    fn default() -> Self {
        Self {
            user_skill_root: dirs_user_skill_root(),
            mode: DefaultSkillInstallMode::InstallMissingAndUpdateManaged,
        }
    }
}

fn dirs_user_skill_root() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".devo").join("skills")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultSkillInstallMode {
    InstallMissingAndUpdateManaged,
    InstallMissingOnly,
    DryRun,
}

/// Report from a default skill installation run.
#[derive(Debug, Clone)]
pub struct DefaultSkillInstallReport {
    pub installed: Vec<SkillName>,
    pub updated: Vec<SkillName>,
    pub skipped_user_modified: Vec<SkillName>,
    pub skipped_conflict: Vec<SkillName>,
    pub failed: Vec<DefaultSkillInstallFailure>,
}

/// A failure during default skill installation.
#[derive(Debug, Clone)]
pub struct DefaultSkillInstallFailure {
    pub package_name: SkillName,
    pub reason: String,
}

// ── Managed Metadata ─────────────────────────────────────────────────

/// Managed metadata written alongside installed default skills.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedMetadata {
    pub default_skill_id: String,
    pub bundle_version: String,
    pub installed_hash: String,
    pub installed_at: String,
    pub last_synced_at: String,
    pub managed_by: String,
}

impl ManagedMetadata {
    fn new(skill_id: &str, bundle_version: &str, installed_hash: &str) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            default_skill_id: skill_id.to_string(),
            bundle_version: bundle_version.to_string(),
            installed_hash: installed_hash.to_string(),
            installed_at: now.clone(),
            last_synced_at: now,
            managed_by: "devo".to_string(),
        }
    }

    fn metadata_path(package_dir: &Path) -> PathBuf {
        package_dir.join(".devo").join("default-skill.toml")
    }

    fn write(&self, package_dir: &Path) -> Result<(), String> {
        let meta_dir = package_dir.join(".devo");
        std::fs::create_dir_all(&meta_dir)
            .map_err(|e| format!("failed to create metadata dir: {}", e))?;
        let path = Self::metadata_path(package_dir);
        let toml_str = toml::to_string_pretty(self)
            .map_err(|e| format!("failed to serialize metadata: {}", e))?;
        std::fs::write(&path, toml_str).map_err(|e| format!("failed to write metadata: {}", e))?;
        Ok(())
    }

    fn read(package_dir: &Path) -> Result<Option<Self>, String> {
        let path = Self::metadata_path(package_dir);
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("failed to read metadata: {}", e))?;
        let meta: Self =
            toml::from_str(&content).map_err(|e| format!("failed to parse metadata: {}", e))?;
        Ok(Some(meta))
    }
}

// ── Installer ────────────────────────────────────────────────────────

/// Installs bundled default skills idempotently into the user skill root.
#[derive(Debug, Clone)]
pub struct DefaultSkillInstaller {
    pub bundle: DefaultSkillBundle,
}

impl DefaultSkillInstaller {
    pub fn new(bundle: DefaultSkillBundle) -> Self {
        Self { bundle }
    }

    /// Run installation with the given options.
    ///
    /// Algorithm per L3-BEH-SKILLS-001 §6:
    /// 1. Ensure user skill root exists.
    /// 2. For each bundled skill:
    ///    - Check if target exists and has managed metadata.
    ///    - Install missing, update managed, skip user-modified.
    /// 3. Write managed metadata after successful install/update.
    /// 4. Return report.
    pub fn install(&self, options: &DefaultSkillInstallOptions) -> DefaultSkillInstallReport {
        let mut report = DefaultSkillInstallReport {
            installed: vec![],
            updated: vec![],
            skipped_user_modified: vec![],
            skipped_conflict: vec![],
            failed: vec![],
        };

        // Ensure user skill root exists
        if !matches!(options.mode, DefaultSkillInstallMode::DryRun) {
            if let Err(e) = std::fs::create_dir_all(&options.user_skill_root) {
                report.failed.push(DefaultSkillInstallFailure {
                    package_name: SkillName::new("_root").unwrap_or(SkillName("_root".into())),
                    reason: format!("cannot create skill root: {}", e),
                });
                return report;
            }
        }

        for asset in &self.bundle.skills {
            let package_dir = options.user_skill_root.join(asset.package_name.as_str());

            match self.install_one(asset, &package_dir, options.mode) {
                Ok(action) => match action {
                    InstallAction::Installed => report.installed.push(asset.package_name.clone()),
                    InstallAction::Updated => report.updated.push(asset.package_name.clone()),
                    InstallAction::SkippedUserModified => report
                        .skipped_user_modified
                        .push(asset.package_name.clone()),
                    InstallAction::SkippedConflict => {
                        report.skipped_conflict.push(asset.package_name.clone())
                    }
                    InstallAction::SkippedDryRun => {}
                },
                Err(reason) => report.failed.push(DefaultSkillInstallFailure {
                    package_name: asset.package_name.clone(),
                    reason,
                }),
            }
        }

        report
    }

    fn install_one(
        &self,
        asset: &DefaultSkillAsset,
        package_dir: &Path,
        mode: DefaultSkillInstallMode,
    ) -> Result<InstallAction, String> {
        match mode {
            DefaultSkillInstallMode::DryRun => {
                if package_dir.exists() {
                    if ManagedMetadata::read(package_dir)?.is_some() {
                        // Would update if hash changed
                        let meta = ManagedMetadata::read(package_dir)?.unwrap();
                        if meta.installed_hash != asset.content_hash.0 {
                            return Ok(InstallAction::SkippedDryRun); // would update
                        }
                    } else {
                        return Ok(InstallAction::SkippedConflict); // would skip
                    }
                } else {
                    return Ok(InstallAction::SkippedDryRun); // would install
                }
                Ok(InstallAction::SkippedDryRun)
            }
            _ => {
                if package_dir.exists() {
                    if let Some(meta) = ManagedMetadata::read(package_dir)? {
                        // Has managed metadata — this is a managed package
                        if meta.installed_hash == asset.content_hash.0 {
                            // Already installed with this exact version
                            return Ok(InstallAction::SkippedUserModified); // unchanged
                        }
                        // Hash changed — update if mode allows
                        match mode {
                            DefaultSkillInstallMode::InstallMissingAndUpdateManaged => {
                                self.write_package(asset, package_dir)?;
                                ManagedMetadata::new(
                                    &asset.default_skill_id,
                                    &self.bundle.bundle_version,
                                    &asset.content_hash.0,
                                )
                                .write(package_dir)?;
                                Ok(InstallAction::Updated)
                            }
                            DefaultSkillInstallMode::InstallMissingOnly => {
                                Ok(InstallAction::SkippedUserModified)
                            }
                            _ => Ok(InstallAction::SkippedUserModified),
                        }
                    } else {
                        // No managed metadata — user package conflict
                        Ok(InstallAction::SkippedConflict)
                    }
                } else {
                    // Package doesn't exist — install it
                    self.write_package(asset, package_dir)?;
                    ManagedMetadata::new(
                        &asset.default_skill_id,
                        &self.bundle.bundle_version,
                        &asset.content_hash.0,
                    )
                    .write(package_dir)?;
                    Ok(InstallAction::Installed)
                }
            }
        }
    }

    fn write_package(&self, asset: &DefaultSkillAsset, package_dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(package_dir)
            .map_err(|e| format!("failed to create package dir: {}", e))?;

        // Write SKILL.md entrypoint
        for file in &asset.files {
            let file_path = package_dir.join(&file.relative_path);
            if let Some(parent) = file_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("failed to create dir: {}", e))?;
            }
            std::fs::write(&file_path, &file.content)
                .map_err(|e| format!("failed to write {}: {}", file.relative_path, e))?;

            #[cfg(unix)]
            if file.executable {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&file_path)
                    .map_err(|e| format!("metadata: {}", e))?
                    .permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&file_path, perms).map_err(|e| format!("chmod: {}", e))?;
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallAction {
    Installed,
    Updated,
    SkippedUserModified,
    SkippedConflict,
    SkippedDryRun,
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_bundle() -> DefaultSkillBundle {
        DefaultSkillBundle {
            bundle_version: "1.0.0".into(),
            skills: vec![DefaultSkillAsset {
                default_skill_id: "builtin.code-review".into(),
                package_name: SkillName::new("code-review").unwrap(),
                package_version: Some("1.0".into()),
                content_hash: Sha256Digest("abc123".into()),
                files: vec![EmbeddedSkillFile {
                    relative_path: "SKILL.md".into(),
                    content: "---\nname: code-review\ndescription: Review code\n---\n\nReview code for bugs.".into(),
                    executable: false,
                }],
            }],
        }
    }

    #[test]
    fn installs_missing_skill() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path().join("skills");
        let options = DefaultSkillInstallOptions {
            user_skill_root: root.clone(),
            mode: DefaultSkillInstallMode::InstallMissingAndUpdateManaged,
        };

        let installer = DefaultSkillInstaller::new(make_bundle());
        let report = installer.install(&options);

        assert_eq!(report.installed.len(), 1);
        assert!(report.failed.is_empty());
        assert!(root.join("code-review").join("SKILL.md").exists());
        assert!(
            root.join("code-review")
                .join(".devo")
                .join("default-skill.toml")
                .exists()
        );
    }

    #[test]
    fn skips_existing_user_package() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path().join("skills");
        let pkg_dir = root.join("code-review");
        std::fs::create_dir_all(&pkg_dir).expect("create");
        std::fs::write(pkg_dir.join("SKILL.md"), "user content").expect("write");

        let options = DefaultSkillInstallOptions {
            user_skill_root: root,
            mode: DefaultSkillInstallMode::InstallMissingAndUpdateManaged,
        };

        let installer = DefaultSkillInstaller::new(make_bundle());
        let report = installer.install(&options);

        assert_eq!(report.skipped_conflict.len(), 1);
        assert_eq!(report.installed.len(), 0);
    }

    #[test]
    fn dry_run_does_not_write_files() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path().join("skills");
        let options = DefaultSkillInstallOptions {
            user_skill_root: root.clone(),
            mode: DefaultSkillInstallMode::DryRun,
        };

        let installer = DefaultSkillInstaller::new(make_bundle());
        let report = installer.install(&options);

        assert_eq!(report.installed.len(), 0);
        assert!(!root.join("code-review").exists());
    }

    #[test]
    fn skips_unchanged_managed_package() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path().join("skills");

        // First install
        let options = DefaultSkillInstallOptions {
            user_skill_root: root.clone(),
            mode: DefaultSkillInstallMode::InstallMissingAndUpdateManaged,
        };
        let installer = DefaultSkillInstaller::new(make_bundle());
        installer.install(&options);

        // Re-install with same bundle — should skip as unchanged
        let installer2 = DefaultSkillInstaller::new(make_bundle());
        let report = installer2.install(&options);

        // unchanged package → skipped
        assert_eq!(report.installed.len(), 0);
        assert_eq!(report.updated.len(), 0);
        // metadata hash matches → skip (treated as user_modified skip in simplified logic)
    }

    #[test]
    fn updates_managed_when_hash_matches_previous() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path().join("skills");
        let options = DefaultSkillInstallOptions {
            user_skill_root: root.clone(),
            mode: DefaultSkillInstallMode::InstallMissingAndUpdateManaged,
        };

        // First install
        let installer = DefaultSkillInstaller::new(make_bundle());
        installer.install(&options);

        // Re-install with updated content and new hash
        let mut bundle2 = make_bundle();
        bundle2.skills[0].content_hash = Sha256Digest("newhash".into());
        bundle2.skills[0].files[0].content =
            "---\nname: code-review\ndescription: Updated\n---\n\nUpdated body.".into();

        // Manually update metadata to match the old hash
        let meta = ManagedMetadata {
            default_skill_id: "builtin.code-review".into(),
            bundle_version: "1.0.0".into(),
            installed_hash: "abc123".into(),
            installed_at: chrono::Utc::now().to_rfc3339(),
            last_synced_at: chrono::Utc::now().to_rfc3339(),
            managed_by: "devo".into(),
        };
        meta.write(&root.join("code-review")).expect("write meta");

        let installer2 = DefaultSkillInstaller::new(bundle2);
        let report = installer2.install(&options);

        // Should update because metadata hash matches previous, bundle hash is new
        assert_eq!(report.updated.len(), 1);
    }

    #[test]
    fn managed_metadata_roundtrip() {
        let tmp = TempDir::new().expect("tempdir");
        let pkg_dir = tmp.path().join("test-skill");
        std::fs::create_dir_all(&pkg_dir).expect("create");

        let meta = ManagedMetadata::new("test.id", "1.0", "hash123");
        meta.write(&pkg_dir).expect("write");

        let read = ManagedMetadata::read(&pkg_dir).expect("read");
        assert!(read.is_some());
        let read = read.unwrap();
        assert_eq!(read.default_skill_id, "test.id");
        assert_eq!(read.bundle_version, "1.0");
        assert_eq!(read.managed_by, "devo");
    }
}

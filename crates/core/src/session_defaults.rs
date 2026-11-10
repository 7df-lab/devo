//! Product defaults for a newly created session, shared by Native and headless CLI.

use std::path::Path;
use std::path::PathBuf;

use devo_config::AppConfig;
use devo_config::project_config_key;
use devo_safety::PermissionPreset;
use devo_safety::RuntimePermissionProfile;

use crate::AgentsMdConfig;
use crate::SessionConfig;

impl SessionConfig {
    /// Resolve new-session product defaults without changing the conservative
    /// low-level `Default`. Explicit project permissions and project/global
    /// sandbox profiles take precedence over Full Access with the sandbox off.
    /// Resume must restore persisted settings after this initial baseline.
    pub fn for_new_session(
        config: &AppConfig,
        cwd: &Path,
        additional_directories: &[PathBuf],
    ) -> Self {
        let project = config.projects.get(&project_config_key(cwd));
        let preset = match project.and_then(|project| project.permission_preset) {
            Some(devo_protocol::PermissionPreset::Default) => PermissionPreset::Default,
            Some(devo_protocol::PermissionPreset::AutoReview) => PermissionPreset::AutoReview,
            Some(devo_protocol::PermissionPreset::FullAccess) | None => {
                PermissionPreset::FullAccess
            }
        };
        let permission_profile = RuntimePermissionProfile::from_preset(preset, cwd.to_path_buf())
            .with_additional_roots(additional_directories.iter().cloned());
        let sandbox_profile = project
            .and_then(|project| project.sandbox_profile.clone())
            .or_else(|| config.permission.sandbox_profile.clone())
            .unwrap_or_else(|| permission_profile.implied_sandbox_profile().to_string());
        Self {
            permission_mode: permission_profile.permission_mode(),
            permission_profile,
            sandbox_profile: Some(sandbox_profile),
            agents_md: AgentsMdConfig {
                project_root_markers: config.project_root_markers.clone(),
                ..AgentsMdConfig::default()
            },
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use devo_config::ProjectConfig;
    use devo_safety::PermissionMode;
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn new_session_permissions_preserve_explicit_configuration() {
        let cwd = std::env::temp_dir().join("devo-session-defaults");
        let additional = cwd.join("additional");
        for (preset, project_sandbox, global_sandbox, expected_preset, expected_sandbox) in [
            (None, None, None, PermissionPreset::FullAccess, "off"),
            (
                Some(devo_protocol::PermissionPreset::Default),
                None,
                None,
                PermissionPreset::Default,
                "workspace",
            ),
            (
                Some(devo_protocol::PermissionPreset::AutoReview),
                None,
                None,
                PermissionPreset::AutoReview,
                "workspace",
            ),
            (
                Some(devo_protocol::PermissionPreset::FullAccess),
                None,
                None,
                PermissionPreset::FullAccess,
                "off",
            ),
            (
                None,
                None,
                Some("strict"),
                PermissionPreset::FullAccess,
                "strict",
            ),
            (
                None,
                Some("read-only"),
                Some("strict"),
                PermissionPreset::FullAccess,
                "read-only",
            ),
            (
                Some(devo_protocol::PermissionPreset::Default),
                Some("off"),
                None,
                PermissionPreset::Default,
                "off",
            ),
        ] {
            let mut config = AppConfig::default();
            config.permission.sandbox_profile = global_sandbox.map(str::to_string);
            config.projects.insert(
                project_config_key(&cwd),
                ProjectConfig {
                    permission_preset: preset,
                    sandbox_profile: project_sandbox.map(str::to_string),
                },
            );
            let session =
                SessionConfig::for_new_session(&config, &cwd, std::slice::from_ref(&additional));
            let expected_profile =
                RuntimePermissionProfile::from_preset(expected_preset, cwd.clone())
                    .with_additional_roots([additional.clone()]);
            assert_eq!(
                (
                    session.permission_mode,
                    session.permission_profile,
                    session.sandbox_profile
                ),
                (
                    expected_profile.permission_mode(),
                    expected_profile,
                    Some(expected_sandbox.to_string())
                ),
            );
        }
    }

    #[test]
    fn low_level_default_remains_restricted() {
        let config = SessionConfig::default();
        assert_eq!(
            (
                config.permission_mode,
                config.permission_profile.preset,
                config.sandbox_profile
            ),
            (
                PermissionMode::Interactive,
                PermissionPreset::Default,
                Some("workspace".to_string())
            ),
        );
    }
}

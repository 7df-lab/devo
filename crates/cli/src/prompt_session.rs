//! Resolve headless session settings before provider or tool execution.

use std::path::Path;

use anyhow::Result;
use devo_core::AppConfig;
use devo_core::SessionConfig;
use devo_safety::PermissionPreset;

pub(crate) fn session_config(config: &AppConfig, cwd: &Path) -> Result<SessionConfig> {
    let session = SessionConfig::for_new_session(config, cwd, &[]);
    if !matches!(
        session.permission_profile.preset,
        PermissionPreset::FullAccess
    ) {
        anyhow::bail!(
            "devo prompt cannot enforce the configured approval preset without an interactive \
             approval controller; use `devo` in this workspace instead. The configured permissions \
             have not been changed, and no model request or tool has run"
        );
    }
    Ok(session)
}

#[cfg(test)]
mod tests {
    use devo_core::ProjectConfig;
    use devo_core::project_config_key;
    use devo_safety::PermissionMode;
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn headless_defaults_to_full_access_without_sandbox() {
        let cwd = std::env::temp_dir().join("devo-prompt-defaults");
        let session = session_config(&AppConfig::default(), &cwd).expect("headless defaults");
        assert_eq!(
            (
                session.permission_mode,
                session.permission_profile.preset,
                session.sandbox_profile
            ),
            (
                PermissionMode::Yolo,
                PermissionPreset::FullAccess,
                Some("off".to_string())
            ),
        );
    }

    #[test]
    fn headless_preserves_global_and_project_sandbox_overrides() {
        let cwd = std::env::temp_dir().join("devo-prompt-sandbox");
        let mut config = AppConfig::default();
        config.permission.sandbox_profile = Some("strict".to_string());
        assert_eq!(
            session_config(&config, &cwd).unwrap().sandbox_profile,
            Some("strict".to_string())
        );
        config.projects.insert(
            project_config_key(&cwd),
            ProjectConfig {
                permission_preset: Some(devo_protocol::PermissionPreset::FullAccess),
                sandbox_profile: Some("read-only".to_string()),
            },
        );
        assert_eq!(
            session_config(&config, &cwd).unwrap().sandbox_profile,
            Some("read-only".to_string())
        );
    }

    #[test]
    fn headless_rejects_approval_presets_instead_of_silently_bypassing_them() {
        let cwd = std::env::temp_dir().join("devo-prompt-approval");
        for preset in [
            devo_protocol::PermissionPreset::Default,
            devo_protocol::PermissionPreset::AutoReview,
        ] {
            let mut config = AppConfig::default();
            config.projects.insert(
                project_config_key(&cwd),
                ProjectConfig {
                    permission_preset: Some(preset),
                    sandbox_profile: Some("off".to_string()),
                },
            );
            let error = session_config(&config, &cwd).expect_err("no headless approval bypass");
            assert!(
                error
                    .to_string()
                    .contains("no model request or tool has run")
            );
            assert_eq!(
                config.projects[&project_config_key(&cwd)].permission_preset,
                Some(preset)
            );
        }
    }
}

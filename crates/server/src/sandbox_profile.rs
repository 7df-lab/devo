//! Session sandbox-profile validation and listing.
//!
//! The active sandbox profile for a session is just a name stored in
//! `SessionConfig::sandbox_profile`; this module validates user-supplied names
//! against the built-in profiles and the custom profiles defined in
//! `sandbox.toml`, and lists the custom names for config-option builders.

use std::path::Path;

use devo_sandbox::ProfileName;
use devo_sandbox::load_sandbox_config;

/// Validates a user-supplied sandbox profile name, returning the canonical
/// name on success. Built-in names and aliases (`readonly`, `none`) are
/// normalized; any other value must name a custom profile defined in
/// `sandbox.toml` for the given workspace.
pub(crate) fn normalize_sandbox_profile_name(value: &str, cwd: &Path) -> Result<String, String> {
    let profile: ProfileName = value.trim().parse().map_err(|error: String| error)?;
    if let ProfileName::Custom(name) = &profile {
        let config = load_sandbox_config(cwd).map_err(|error| error.to_string())?;
        if !config.profiles.contains_key(name) {
            return Err(format!(
                "unknown sandbox profile '{name}': not a built-in profile and not defined in sandbox.toml"
            ));
        }
    }
    Ok(profile.to_string())
}

/// Sorted custom profile names defined in `sandbox.toml` for the workspace.
/// Returns an empty list when the config is missing or cannot be loaded.
///
/// Kept for advanced/API callers even though the ACP session config option no
/// longer surfaces sandbox profiles in the interactive UI.
#[allow(dead_code)]
pub(crate) fn custom_sandbox_profile_names(cwd: &Path) -> Vec<String> {
    let mut names: Vec<String> = match load_sandbox_config(cwd) {
        Ok(config) => config
            .profiles
            .into_keys()
            .filter(|name| matches!(name.parse(), Ok(ProfileName::Custom(_))))
            .collect(),
        Err(error) => {
            tracing::warn!(
                %error,
                cwd = %cwd.display(),
                "failed to load sandbox.toml while listing custom sandbox profiles"
            );
            Vec::new()
        }
    };
    names.sort_unstable();
    names
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn normalize_builtin_profile_names() {
        let cwd = std::env::current_dir().expect("current dir");
        for (input, expected) in [
            ("workspace", "workspace"),
            ("devbox", "devbox"),
            ("read-only", "read-only"),
            ("readonly", "read-only"),
            ("strict", "strict"),
            ("off", "off"),
            ("none", "off"),
            (" workspace ", "workspace"),
        ] {
            assert_eq!(
                normalize_sandbox_profile_name(input, &cwd).as_deref(),
                Ok(expected),
                "input: {input}"
            );
        }
    }

    #[test]
    fn normalize_rejects_unknown_custom_profile() {
        let cwd =
            std::env::temp_dir().join(format!("devo-sandbox-profile-test-{}", std::process::id()));
        std::fs::create_dir_all(&cwd).expect("create test workspace");

        let error = normalize_sandbox_profile_name("definitely-not-a-profile", &cwd)
            .expect_err("unknown custom profile must be rejected");

        assert!(
            error.contains("definitely-not-a-profile"),
            "unexpected error: {error}"
        );

        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn custom_sandbox_profile_names_lists_project_profiles() {
        let cwd = std::env::temp_dir().join(format!(
            "devo-sandbox-profile-list-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(cwd.join(".devo")).expect("create test workspace");
        std::fs::write(
            cwd.join(".devo").join("sandbox.toml"),
            "[profiles.team-ci]\nextends = \"workspace\"\n\n[profiles.workspace]\nrestrict_network = true\n",
        )
        .expect("write project sandbox.toml");

        let names = custom_sandbox_profile_names(&cwd);

        // `workspace` is a built-in name and must not be listed as custom.
        // The global sandbox.toml may legitimately define other customs, so
        // assert membership rather than exact equality.
        assert!(names.contains(&"team-ci".to_string()));
        assert!(!names.contains(&"workspace".to_string()));

        let _ = std::fs::remove_dir_all(&cwd);
    }
}

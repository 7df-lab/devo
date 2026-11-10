//! Headless startup must not silently replace explicitly restrictive settings.

use std::process::Command;

use tempfile::TempDir;

#[test]
fn configured_approval_preset_fails_before_provider_initialization() {
    for preset in ["default", "auto-review"] {
        let home = TempDir::new().expect("home");
        let workspace = TempDir::new().expect("workspace");
        let project_key = devo_core::project_config_key(workspace.path());
        let config = toml::toml! {
            [updates]
            enabled = false
            check_on_startup = false
            [projects]
        };
        let mut config = config;
        config["projects"]
            .as_table_mut()
            .expect("projects table")
            .insert(
                project_key,
                toml::Value::Table(toml::Table::from_iter([
                    (
                        "permission_preset".to_string(),
                        toml::Value::String(preset.to_string()),
                    ),
                    (
                        "sandbox_profile".to_string(),
                        toml::Value::String("off".to_string()),
                    ),
                ])),
            );
        std::fs::write(
            home.path().join("config.toml"),
            toml::to_string(&config).unwrap(),
        )
        .unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_devo"))
            .env("DEVO_HOME", home.path())
            .current_dir(workspace.path())
            .args(["prompt", "Do not execute anything"])
            .output()
            .expect("run headless CLI");
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("no model request or tool has run"),
            "preset {preset}: {}",
            String::from_utf8_lossy(&output.stderr),
        );
    }
}

#[test]
fn invalid_configuration_does_not_fall_back_to_full_access() {
    let home = TempDir::new().expect("home");
    let workspace = TempDir::new().expect("workspace");
    std::fs::write(
        home.path().join("config.toml"),
        "[permission]\nsandbox_profile = 42\n",
    )
    .expect("write invalid config");
    let output = Command::new(env!("CARGO_BIN_EXE_devo"))
        .env("DEVO_HOME", home.path())
        .current_dir(workspace.path())
        .args(["prompt", "Do not execute anything"])
        .output()
        .expect("run headless CLI");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("failed to load prompt configuration"),
        "{}",
        String::from_utf8_lossy(&output.stderr),
    );
}

//! Builds argv/env launch descriptions for the legacy restricted-token wrapper path.

use crate::WindowsSandboxLaunch;
use crate::WindowsSandboxProxySettingsMode;
use crate::WindowsSandboxRequest;
use crate::protocol::config_types::WindowsSandboxLevel;
use crate::request_adapter::deny_read_overrides;
use crate::request_adapter::permission_profile_from_request;
use crate::request_adapter::workspace_roots_from_request;
use crate::wrapper::create_windows_sandbox_command_args_for_permission_profile;
use devo_util_paths::find_devo_home;
use std::collections::HashMap;
use std::env;

pub(crate) fn prepare_launch(req: &WindowsSandboxRequest) -> anyhow::Result<WindowsSandboxLaunch> {
    let mut inner_command = vec![req.shell_program.clone()];
    inner_command.extend(req.shell_args.iter().cloned());
    inner_command.push(req.command.clone());
    let permission_profile = permission_profile_from_request(req)?;
    let workspace_roots = workspace_roots_from_request(req)?;
    let command_cwd = workspace_roots
        .first()
        .cloned()
        .expect("workspace_roots_from_request always returns at least cwd");
    let deny_read_paths_override = deny_read_overrides(req)?;
    let devo_home = find_devo_home()?;

    let env_map = compose_launch_env(env::vars().collect(), &req.env_extra);
    let current_exe = env::current_exe()?;
    // Library integration tests live in Cargo's `debug/deps` directory and do
    // not implement the CLI's sandbox early-dispatch hook. Use the adjacent
    // built CLI for that layout, rather than relaunching the test harness.
    let program = current_exe
        .parent()
        .filter(|parent| parent.file_name() == Some(std::ffi::OsStr::new("deps")))
        .and_then(std::path::Path::parent)
        .map(|parent| parent.join("devo.exe"))
        .filter(|candidate| candidate.is_file())
        .unwrap_or(current_exe);
    let args = create_windows_sandbox_command_args_for_permission_profile(
        inner_command,
        &command_cwd,
        workspace_roots.as_slice(),
        &env_map,
        &permission_profile,
        // Network restriction requires the elevated backend's account-level
        // firewall; a legacy restricted token alone does not block outbound.
        if req.restrict_network {
            WindowsSandboxLevel::Elevated
        } else {
            WindowsSandboxLevel::RestrictedToken
        },
        /*windows_sandbox_private_desktop*/ false,
        /*proxy_enforced*/ false,
        WindowsSandboxProxySettingsMode::Reconcile,
        Some(req.readable_roots.as_slice()),
        /*read_roots_include_platform_defaults*/ false,
        Some(req.writable_roots.as_slice()),
        deny_read_paths_override.as_slice(),
        &[],
        devo_home.as_path(),
    );

    Ok(WindowsSandboxLaunch {
        program,
        args,
        env: env_map.into_iter().collect(),
    })
}

fn compose_launch_env(
    inherited_env: Vec<(String, String)>,
    extra_env: &[(String, String)],
) -> HashMap<String, String> {
    let mut env_map = inherited_env.into_iter().collect::<HashMap<_, _>>();
    // Explicit child overrides take precedence over inherited values.
    for (key, value) in extra_env {
        env_map.insert(key.clone(), value.clone());
    }
    env_map
}

#[cfg(test)]
mod tests {
    use super::compose_launch_env;
    use pretty_assertions::assert_eq;

    #[test]
    fn explicit_child_env_overrides_inherited_values() {
        let env_map = compose_launch_env(
            vec![
                ("PATH".into(), r"C:\Python".into()),
                ("MODE".into(), "old".into()),
            ],
            &[("MODE".into(), "new".into())],
        );
        assert_eq!(
            env_map,
            std::collections::HashMap::from([
                ("PATH".into(), r"C:\Python".into()),
                ("MODE".into(), "new".into()),
            ])
        );
    }
}

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
    let permission_profile = permission_profile_from_request(req)?;
    let workspace_roots = workspace_roots_from_request(req)?;
    let command_cwd = workspace_roots
        .first()
        .cloned()
        .expect("workspace_roots_from_request always returns at least cwd");
    let deny_read_paths_override = deny_read_overrides(req)?;
    let devo_home = find_devo_home()?;

    let mut inner_command = vec![req.shell_program.clone()];
    inner_command.extend(req.shell_args.iter().cloned());
    inner_command.push(req.command.clone());

    let env_map = env::vars().collect::<HashMap<_, _>>();
    let program = env::current_exe()?;
    let args = create_windows_sandbox_command_args_for_permission_profile(
        inner_command,
        &command_cwd,
        workspace_roots.as_slice(),
        &env_map,
        &permission_profile,
        WindowsSandboxLevel::RestrictedToken,
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

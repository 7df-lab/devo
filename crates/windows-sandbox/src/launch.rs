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
    prepare_direct_argv_launch_with_env(req, inner_command, env::vars().collect())
}

/// Direct-argv variant for callers that exec a specific argv (not a shell
/// command string), e.g. the RLM kernel host spawning `python -m rlm.repl`.
pub(crate) fn prepare_direct_argv_launch(
    req: &WindowsSandboxRequest,
    inner_command: Vec<String>,
) -> anyhow::Result<WindowsSandboxLaunch> {
    prepare_direct_argv_launch_with_env(req, inner_command, env::vars().collect())
}

/// A caller-supplied inherited environment is necessary for the kernel: its
/// credential filter must run BEFORE we serialize this map in wrapper argv.
pub(crate) fn prepare_direct_argv_launch_with_env(
    req: &WindowsSandboxRequest,
    inner_command: Vec<String>,
    inherited_env: Vec<(String, String)>,
) -> anyhow::Result<WindowsSandboxLaunch> {
    let permission_profile = permission_profile_from_request(req)?;
    let workspace_roots = workspace_roots_from_request(req)?;
    let command_cwd = workspace_roots
        .first()
        .cloned()
        .expect("workspace_roots_from_request always returns at least cwd");
    let deny_read_paths_override = deny_read_overrides(req)?;
    let devo_home = find_devo_home()?;

    let env_map = compose_launch_env(inherited_env, &req.env_extra);
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
        // Network-restricted fences need the elevated backend: the legacy
        // restricted token carries no account-level firewall, so outbound
        // stays open (verified: fenced kernel reached 1.1.1.1:443). The
        // elevated backend runs the child as DevoSandboxOffline, whose
        // firewall rules block non-loopback outbound.
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
        req.session_credential_sid.as_deref(),
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
    // Explicit kernel overrides remain intentional, including values whose
    // names resemble secrets. The kernel host filters only inherited values.
    for (key, value) in extra_env {
        env_map.insert(key.clone(), value.clone());
    }
    env_map
}

#[cfg(test)]
mod tests {
    use super::compose_launch_env;

    #[test]
    fn filtered_kernel_env_is_not_restored_from_parent() {
        // `OPENAI_API_KEY` was filtered out by the kernel before this boundary.
        let env_map = compose_launch_env(
            vec![("PATH".into(), r"C:\Python".into())],
            &[("PYTHONUTF8".into(), "1".into())],
        );
        assert_eq!(env_map.get("PATH").map(String::as_str), Some(r"C:\Python"));
        assert_eq!(env_map.get("PYTHONUTF8").map(String::as_str), Some("1"));
        assert!(!env_map.contains_key("OPENAI_API_KEY"));
        // The exact map is also serialized into wrapper --env-json argv.
        let serialized = serde_json::to_string(&env_map).expect("env JSON");
        assert!(!serialized.contains("OPENAI_API_KEY"));
    }
}

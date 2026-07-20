//! Linux sandbox helper entry point.
//!
//! Outer invocation: parse `--permission-profile` JSON, wrap the command in
//! bwrap (via [`devo_sandbox::wrap_command_for_profile`]), and re-exec this
//! helper with `--apply-seccomp-then-exec` as the command inside bwrap.
//!
//! Inner invocation (`--apply-seccomp-then-exec`): apply the resolved Landlock
//! / seccomp plan, then `execvp` the user command.

/// Entry point for the helper binary / argv0 alias. Never returns.
pub fn run_main() -> ! {
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("devo-linux-sandbox is only supported on Linux");
        std::process::exit(1);
    }
    #[cfg(target_os = "linux")]
    linux::run_main_linux()
}

#[cfg(target_os = "linux")]
mod linux {
    use std::ffi::CString;
    use std::path::PathBuf;
    use std::process::Command;

    use clap::Parser;
    use devo_sandbox::LinuxSandboxPermissionProfile;
    use devo_sandbox::SandboxLogger;
    use devo_sandbox::WrapMode;
    use devo_sandbox::apply_resolved_enforcement_in_child;
    use devo_sandbox::resolve_enforcement_plan;
    use devo_sandbox::wrap_command_for_profile;

    /// CLI surface for the Linux sandbox helper.
    #[derive(Debug, Parser)]
    #[command(name = "devo-linux-sandbox", about = "Devo Linux sandbox helper")]
    struct HelperArgs {
        #[arg(long = "sandbox-policy-cwd")]
        sandbox_policy_cwd: PathBuf,

        #[arg(long = "command-cwd")]
        command_cwd: Option<PathBuf>,

        #[arg(long = "permission-profile", value_parser = parse_permission_profile)]
        permission_profile: LinuxSandboxPermissionProfile,

        #[arg(long = "allow-network-for-proxy", default_value_t = false)]
        allow_network_for_proxy: bool,

        /// Internal: apply Landlock/seccomp in the already-sandboxed process, then
        /// Exec the user command after applying seccomp (`--apply-seccomp-then-exec`).
        #[arg(long = "apply-seccomp-then-exec", default_value_t = false)]
        apply_seccomp_then_exec: bool,

        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    }

    fn parse_permission_profile(raw: &str) -> Result<LinuxSandboxPermissionProfile, String> {
        serde_json::from_str(raw).map_err(|error| error.to_string())
    }

    pub(super) fn run_main_linux() -> ! {
        let mut args = HelperArgs::parse();
        if args.allow_network_for_proxy {
            args.permission_profile.allow_network_for_proxy = true;
        }
        if let Err(error) = run_helper(args) {
            eprintln!("devo-linux-sandbox: {error:#}");
            std::process::exit(1);
        }
        std::process::exit(0);
    }

    fn run_helper(args: HelperArgs) -> anyhow::Result<()> {
        let command_cwd = args
            .command_cwd
            .clone()
            .unwrap_or_else(|| args.sandbox_policy_cwd.clone());
        if args.command.is_empty() {
            anyhow::bail!("missing command after --");
        }

        if args.apply_seccomp_then_exec {
            return apply_then_exec(&args.permission_profile, &args.command);
        }

        // Outer: wrap this helper (apply-seccomp mode) + user command in bwrap.
        let mut inner_command = vec![
            std::env::current_exe()?.to_string_lossy().into_owned(),
            "--sandbox-policy-cwd".to_string(),
            args.sandbox_policy_cwd.to_string_lossy().into_owned(),
            "--command-cwd".to_string(),
            command_cwd.to_string_lossy().into_owned(),
            "--permission-profile".to_string(),
            serde_json::to_string(&args.permission_profile)?,
            "--apply-seccomp-then-exec".to_string(),
            "--".to_string(),
        ];
        inner_command.extend(args.command.iter().cloned());

        // Avoid recursively selecting the helper wrap: force direct bwrap.
        // SAFETY: single-threaded helper process before any spawn.
        let previous_override = std::env::var_os("DEVO_SANDBOX_LAUNCHER");
        unsafe {
            std::env::set_var("DEVO_SANDBOX_LAUNCHER", "bwrap");
            std::env::remove_var("DEVO_LINUX_SANDBOX");
        }

        let wrap = wrap_command_for_profile(
            Some(args.permission_profile.profile.as_str()),
            &args.permission_profile.workspace,
            WrapMode::PtyOnly,
            &SandboxLogger::new(),
        )?;

        match previous_override {
            Some(value) => unsafe { std::env::set_var("DEVO_SANDBOX_LAUNCHER", value) },
            None => unsafe { std::env::remove_var("DEVO_SANDBOX_LAUNCHER") },
        }

        match wrap {
            devo_sandbox::SandboxWrap::Wrapped(wrapped) => {
                let mut argv = wrapped.prefix_args;
                argv.extend(inner_command);
                let status = Command::new(&wrapped.program)
                    .args(&argv)
                    .current_dir(&command_cwd)
                    .status()?;
                if !status.success() {
                    std::process::exit(status.code().unwrap_or(1));
                }
                Ok(())
            }
            devo_sandbox::SandboxWrap::None => {
                // No bwrap available: still apply Landlock/seccomp then exec.
                apply_then_exec(&args.permission_profile, &args.command)
            }
        }
    }

    fn apply_then_exec(
        permission_profile: &LinuxSandboxPermissionProfile,
        command: &[String],
    ) -> anyhow::Result<()> {
        let plan = resolve_enforcement_plan(
            Some(permission_profile.profile.as_str()),
            &permission_profile.workspace,
        )?;
        apply_resolved_enforcement_in_child(plan.as_ref())?;
        exec_command(command)
    }

    fn exec_command(command: &[String]) -> anyhow::Result<()> {
        let program = &command[0];
        let c_program = CString::new(program.as_bytes())
            .map_err(|error| anyhow::anyhow!("invalid program path: {error}"))?;
        let c_args: Vec<CString> = command
            .iter()
            .map(|arg| {
                CString::new(arg.as_bytes())
                    .map_err(|error| anyhow::anyhow!("invalid command argument: {error}"))
            })
            .collect::<anyhow::Result<_>>()?;
        let mut argv_ptrs: Vec<*const libc::c_char> = c_args.iter().map(|c| c.as_ptr()).collect();
        argv_ptrs.push(std::ptr::null());
        // SAFETY: argv_ptrs are valid NUL-terminated C strings for the duration of
        // the call. On success execvp does not return.
        unsafe {
            libc::execvp(c_program.as_ptr(), argv_ptrs.as_ptr());
        }
        Err(anyhow::anyhow!(
            "execvp failed for {program}: {}",
            std::io::Error::last_os_error()
        ))
    }
}

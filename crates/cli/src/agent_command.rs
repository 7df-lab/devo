//! Interactive coding-agent entry — product TUI is InteractiveMode (Node).

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use devo_core::SessionId;

use crate::app_exit::AppExit;

/// Resolve `apps/tui/src/index.ts` from the source tree or relative to the
/// running binary (`target/debug/devo` → repo root).
fn resolve_interactive_mode_entry() -> Result<PathBuf> {
    let candidates = [
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/tui/src/index.ts"),
        std::env::current_exe()
            .ok()
            .and_then(|exe| {
                // target/debug/devo.exe → repo root is ../../..
                exe.parent()
                    .and_then(|p| p.parent())
                    .and_then(|p| p.parent())
                    .map(|root| root.join("apps/tui/src/index.ts"))
            })
            .unwrap_or_default(),
    ];
    for candidate in candidates {
        if candidate.as_os_str().is_empty() {
            continue;
        }
        if let Ok(real) = candidate.canonicalize()
            && real.is_file()
        {
            return Ok(simplify_windows_path(real));
        }
        if candidate.is_file() {
            return Ok(simplify_windows_path(candidate));
        }
    }
    anyhow::bail!(
        "InteractiveMode client not found (looked under crates/cli and relative to current exe)."
    )
}

/// Resolve `tsx` CLI next to the TUI package, then fall back to PATH.
fn resolve_tsx_entry(tui_entry: &Path) -> Result<PathBuf> {
    let tui_root = tui_entry
        .parent()
        .and_then(|p| p.parent())
        .ok_or_else(|| anyhow::anyhow!("invalid InteractiveMode entry path"))?;
    let local = tui_root.join("node_modules/tsx/dist/cli.mjs");
    if local.is_file() {
        return Ok(simplify_windows_path(local));
    }
    anyhow::bail!("tsx not found. Run `npm install` in apps/tui.")
}

/// Strip `\\?\` extended prefix so Node and child tools see a normal path.
fn simplify_windows_path(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let raw = path.to_string_lossy();
        if let Some(stripped) = raw.strip_prefix(r"\\?\") {
            return PathBuf::from(stripped);
        }
    }
    path
}

/// Windows: `Command::new("node")` often resolves to `node.cmd`, which mangles
/// backslash paths (e.g. `C:\Users\...` → `C:`). Prefer `node.exe` and pass the
/// script with forward slashes.
fn resolve_node_program() -> PathBuf {
    #[cfg(windows)]
    {
        if let Ok(output) = Command::new("where.exe").arg("node.exe").output()
            && output.status.success()
            && let Some(line) = String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty() && l.to_ascii_lowercase().ends_with("node.exe"))
        {
            return PathBuf::from(line);
        }
        PathBuf::from("node.exe")
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("node")
    }
}

fn path_for_node_arg(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Env var read by `apps/tui` to resume a session instead of `session/new`.
pub(crate) const DEVO_RESUME_SESSION_ID_ENV: &str = "DEVO_RESUME_SESSION_ID";

/// Launch the InteractiveMode client (`apps/tui`) under a TTY.
pub(crate) fn launch_interactive_mode_client(
    resume_session_id: Option<&SessionId>,
    dangerously_skip_permissions: bool,
    onboarding_only: bool,
    log_level: Option<&str>,
) -> Result<()> {
    if !std::io::stdout().is_terminal() {
        anyhow::bail!(
            "InteractiveMode requires a TTY. Run under a real terminal or tmux/psmux (devo-tui-test)."
        );
    }

    let client_entry = resolve_interactive_mode_entry()?;
    let tsx_cli = resolve_tsx_entry(&client_entry)?;
    let node = resolve_node_program();
    let tsx = path_for_node_arg(&tsx_cli);
    let script = path_for_node_arg(&client_entry);
    let server_bin = simplify_windows_path(std::env::current_exe()?);

    let mut command = Command::new(&node);
    command
        .arg(&tsx)
        .arg(&script)
        .env("DEVO_SERVER_BIN", server_bin.as_os_str())
        .env("DEVO_VERSION", env!("CARGO_PKG_VERSION"))
        .env("DEVO_TUI_MAIN", "1");
    if let Some(session_id) = resume_session_id {
        command.env(DEVO_RESUME_SESSION_ID_ENV, session_id.to_string());
    } else {
        // Avoid leaking a stale resume id from the parent shell into a fresh session.
        command.env_remove(DEVO_RESUME_SESSION_ID_ENV);
    }
    // This CLI flag is an explicit request for full access. Do not inherit a
    // stale override from the shell when the flag was not supplied.
    if dangerously_skip_permissions {
        command.env("DEVO_CLI_FULL_ACCESS", "1");
    } else {
        command.env_remove("DEVO_CLI_FULL_ACCESS");
    }
    if onboarding_only {
        command.env("DEVO_CLI_ONBOARDING_ONLY", "1");
    } else {
        command.env_remove("DEVO_CLI_ONBOARDING_ONLY");
    }
    if let Some(level) = log_level {
        command.env("DEVO_CLI_LOG_LEVEL", level);
    } else {
        command.env_remove("DEVO_CLI_LOG_LEVEL");
    }

    let status = command.status().with_context(|| {
        format!(
            "failed to spawn {} for InteractiveMode ({tsx} {script})",
            node.display()
        )
    })?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("InteractiveMode exited with {status}")
    }
}

/// Runs the interactive product TUI (Devo InteractiveMode on Native).
///
/// Legacy `crates/tui` has been removed. The CLI forwards explicit onboarding
/// and full-access requests to the Native-backed InteractiveMode client.
pub(crate) async fn run_agent(
    force_onboarding: bool,
    exit_after_onboarding: bool,
    log_level: Option<&str>,
    initial_session_id: Option<SessionId>,
    dangerously_skip_permissions: bool,
) -> Result<AppExit> {
    tracing::info!(
        resume = initial_session_id
            .as_ref()
            .map(|id| id.to_string())
            .as_deref(),
        "starting InteractiveMode (product TUI)"
    );
    let onboarding_only = force_onboarding && exit_after_onboarding;
    launch_interactive_mode_client(
        initial_session_id.as_ref(),
        dangerously_skip_permissions,
        onboarding_only,
        log_level,
    )?;
    Ok(AppExit {
        onboarding_completed: onboarding_only,
        ..AppExit::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn path_for_node_arg_uses_forward_slashes() {
        let p = PathBuf::from(r"C:\Users\lenovo\Desktop\devo\apps\tui\src\index.ts");
        assert_eq!(
            path_for_node_arg(&p),
            "C:/Users/lenovo/Desktop/devo/apps/tui/src/index.ts"
        );
    }

    #[cfg(windows)]
    #[test]
    fn simplify_windows_path_strips_extended_prefix() {
        let p = PathBuf::from(r"\\?\C:\Users\lenovo\Desktop\devo\apps\tui\src\index.ts");
        assert_eq!(
            simplify_windows_path(p),
            PathBuf::from(r"C:\Users\lenovo\Desktop\devo\apps\tui\src\index.ts")
        );
    }
}

use std::ffi::OsStr;
use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;
use std::process::Output;
use std::process::Stdio;

use tokio::process::Command;

use crate::contracts::ToolCallError;
use crate::contracts::ToolContext;

pub(crate) const RG_NO_MATCH_EXIT_CODE: i32 = 1;

pub(crate) async fn run_rg(
    ctx: &ToolContext,
    args: impl IntoIterator<Item = OsString>,
) -> Result<Output, ToolCallError> {
    let rg = resolve_rg_binary()?;
    let mut command = Command::new(&rg);
    command
        .args(args)
        .current_dir(&ctx.workspace_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let child = command.spawn().map_err(|error| {
        ToolCallError::ExecutionFailed(format!(
            "failed to start ripgrep at {}: {error}",
            rg.display()
        ))
    })?;

    tokio::select! {
        output = child.wait_with_output() => output.map_err(|error| {
            ToolCallError::ExecutionFailed(format!("failed to run ripgrep: {error}"))
        }),
        _ = ctx.cancel_token.cancelled() => Err(ToolCallError::Cancelled),
    }
}

pub(crate) fn resolve_rg_binary() -> Result<PathBuf, ToolCallError> {
    let current_exe = std::env::current_exe().ok();
    let path_env = std::env::var_os("PATH");
    resolve_rg_binary_from(current_exe.as_deref(), path_env.as_deref()).ok_or_else(|| {
        ToolCallError::NeedsConfiguration(format!(
            "ripgrep ({}) was not found next to the devo binary or on PATH. Re-run the devo installer or install ripgrep.",
            rg_binary_name()
        ))
    })
}

pub(crate) fn ripgrep_failure_message(stderr: Vec<u8>, exit_code: i32) -> String {
    let stderr = match String::from_utf8(stderr) {
        Ok(mut value) => {
            trim_in_place(&mut value);
            value
        }
        Err(source) => String::from_utf8_lossy(source.as_bytes())
            .trim()
            .to_string(),
    };

    if stderr.is_empty() {
        format!("ripgrep exited with status {exit_code}")
    } else {
        stderr
    }
}

fn resolve_rg_binary_from(current_exe: Option<&Path>, path_env: Option<&OsStr>) -> Option<PathBuf> {
    if let Some(current_exe) = current_exe
        && let Some(parent) = current_exe.parent()
    {
        let sibling = parent.join(rg_binary_name());
        if sibling.is_file() {
            return Some(sibling);
        }
    }

    let path_env = path_env?;
    std::env::split_paths(path_env)
        .map(|entry| entry.join(rg_binary_name()))
        .find(|candidate| candidate.is_file())
}

fn rg_binary_name() -> &'static str {
    if cfg!(windows) { "rg.exe" } else { "rg" }
}

fn trim_in_place(value: &mut String) {
    let end = value.trim_end().len();
    value.truncate(end);
    let start = value.len().saturating_sub(value.trim_start().len());
    if start > 0 {
        value.drain(..start);
    }
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;

    use pretty_assertions::assert_eq;

    use super::*;

    fn touch(path: &Path) {
        fs::write(path, b"").expect("write executable placeholder");
    }

    #[test]
    fn resolver_prefers_sibling_rg_over_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let bin_dir = temp.path().join("bin");
        let path_dir = temp.path().join("path");
        fs::create_dir_all(&bin_dir).expect("create bin dir");
        fs::create_dir_all(&path_dir).expect("create path dir");

        let sibling = bin_dir.join(rg_binary_name());
        let path_rg = path_dir.join(rg_binary_name());
        touch(&sibling);
        touch(&path_rg);

        let current_exe = bin_dir.join(if cfg!(windows) { "devo.exe" } else { "devo" });
        let path_env = env::join_paths([path_dir]).expect("join path");

        assert_eq!(
            resolve_rg_binary_from(Some(&current_exe), Some(path_env.as_os_str())),
            Some(sibling)
        );
    }

    #[test]
    fn resolver_uses_path_when_sibling_is_missing() {
        let temp = tempfile::tempdir().expect("tempdir");
        let bin_dir = temp.path().join("bin");
        let path_dir = temp.path().join("path");
        fs::create_dir_all(&bin_dir).expect("create bin dir");
        fs::create_dir_all(&path_dir).expect("create path dir");

        let path_rg = path_dir.join(rg_binary_name());
        touch(&path_rg);

        let current_exe = bin_dir.join(if cfg!(windows) { "devo.exe" } else { "devo" });
        let path_env = env::join_paths([path_dir]).expect("join path");

        assert_eq!(
            resolve_rg_binary_from(Some(&current_exe), Some(path_env.as_os_str())),
            Some(path_rg)
        );
    }

    #[test]
    fn ripgrep_failure_message_preserves_stderr_text() {
        assert_eq!(
            ripgrep_failure_message(b"  regex parse error\n".to_vec(), 2),
            "regex parse error"
        );
    }

    #[test]
    fn ripgrep_failure_message_falls_back_to_status() {
        assert_eq!(
            ripgrep_failure_message(b" \n\t".to_vec(), 2),
            "ripgrep exited with status 2"
        );
    }
}

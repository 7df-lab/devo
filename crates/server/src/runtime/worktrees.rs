//! Native Git worktree management. No shell snippets or force deletion: Git
//! owns registration/removal, and reset requires a clean linked checkout.
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use devo_protocol::native::rpc_worktree::{
    WorktreeCreateParams, WorktreeInfo, WorktreeListParams, WorktreeListResult,
    WorktreeMutationParams, WorktreeMutationResult,
};
use serde_json::Value;
use tokio::process::Command;

use super::ServerRuntime;
use crate::{ProtocolErrorCode, SuccessResponse};

// Only worktree mutations serialize here; session and runtime locks are never held.
static MUTATION_GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(super) enum WorktreeOperation {
    List,
    Create,
    Remove,
    Reset,
}

impl ServerRuntime {
    pub(super) async fn handle_native_worktree(
        &self,
        request_id: Value,
        params: Value,
        operation: WorktreeOperation,
    ) -> Value {
        if matches!(
            operation,
            WorktreeOperation::Remove | WorktreeOperation::Reset
        ) && let Some(directory) = params.get("directory").and_then(Value::as_str)
            && let Ok(directory) = tokio::fs::canonicalize(directory).await
        {
            for handle in self.list_session_handles().await {
                if let Some(summary) = handle.summary().await
                    && let Ok(cwd) = tokio::fs::canonicalize(&summary.cwd).await
                    && cwd.starts_with(&directory)
                    && self.runtime_active_turn_id(summary.id).await.is_some()
                {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::PolicyDenied,
                        "Stop working sessions in this worktree before removing or resetting it",
                    );
                }
            }
        }
        let home = self
            .deps
            .config_store
            .lock()
            .expect("config store")
            .user_config_dir()
            .to_path_buf();
        match execute(&home, params, operation).await {
            Ok(result) => serde_json::to_value(SuccessResponse {
                id: request_id,
                result,
            })
            .expect("serialize worktree response"),
            Err(error) => self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                format!("{error:#}"),
            ),
        }
    }
}

async fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    let mut command = Command::new("git");
    // The stdio server owns stdin. Git for Windows otherwise inherits that
    // pipe and can block the Native read loop while probing its handles.
    command
        .current_dir(cwd)
        .args(args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let output = tokio::time::timeout(Duration::from_secs(60), command.output())
        .await
        .context("Git operation timed out")?
        .context("start Git")?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    String::from_utf8(output.stdout).context("Git output is not UTF-8")
}

async fn registered_worktrees(cwd: &Path) -> Result<Vec<WorktreeInfo>> {
    let output = git(cwd, &["worktree", "list", "--porcelain", "-z"]).await?;
    let mut entries = Vec::new();
    for record in output.split("\0\0") {
        let mut directory = None;
        let mut branch = String::from("HEAD");
        for field in record.split('\0') {
            if let Some(value) = field.strip_prefix("worktree ") {
                directory = Some(value.to_string());
            } else if let Some(value) = field.strip_prefix("branch refs/heads/") {
                branch = value.to_string();
            }
        }
        if let Some(directory) = directory {
            let name = Path::new(&directory)
                .file_name()
                .context("worktree name")?
                .to_string_lossy()
                .into_owned();
            entries.push(WorktreeInfo {
                directory,
                name,
                branch,
            });
        }
    }
    Ok(entries)
}

async fn execute(home: &Path, params: Value, operation: WorktreeOperation) -> Result<Value> {
    match operation {
        WorktreeOperation::List => {
            let params: WorktreeListParams = serde_json::from_value(params)?;
            let cwd = Path::new(&params.cwd);
            // Plain folders are valid desktop projects and have no worktrees.
            // Still reject missing paths and report errors from actual repositories.
            tokio::fs::metadata(cwd)
                .await
                .context("read project folder")?;
            let worktrees = if devo_util_git::get_git_repo_root(cwd).is_some() {
                registered_worktrees(cwd)
                    .await?
                    .into_iter()
                    .skip(1)
                    .collect()
            } else {
                Vec::new()
            };
            Ok(serde_json::to_value(WorktreeListResult { worktrees })?)
        }
        WorktreeOperation::Create => {
            let params: WorktreeCreateParams = serde_json::from_value(params)?;
            if params.name.is_empty()
                || params.name.len() > 64
                || !params
                    .name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
            {
                bail!("Worktree name must contain 1–64 letters, numbers, hyphens or underscores");
            }
            let cwd = Path::new(&params.cwd);
            let _guard = MUTATION_GATE.lock().await;
            git(cwd, &["rev-parse", "--show-toplevel"]).await?;
            let suffix = uuid::Uuid::new_v4().simple().to_string();
            let suffix = &suffix[..8];
            let name = format!("{}-{suffix}", params.name);
            let branch = format!("devo/{name}");
            let root = home.join("worktrees");
            tokio::fs::create_dir_all(&root).await?;
            let directory = root.join(&name);
            let directory = directory.to_str().context("worktree path is not UTF-8")?;
            git(
                cwd,
                &["worktree", "add", "-b", &branch, "--", directory, "HEAD"],
            )
            .await?;
            Ok(serde_json::to_value(WorktreeInfo {
                directory: directory.to_string(),
                name,
                branch,
            })?)
        }
        WorktreeOperation::Remove | WorktreeOperation::Reset => {
            let params: WorktreeMutationParams = serde_json::from_value(params)?;
            let cwd = Path::new(&params.cwd);
            let _guard = MUTATION_GATE.lock().await;
            let requested = tokio::fs::canonicalize(&params.directory)
                .await
                .context("resolve worktree path")?;
            let registered = registered_worktrees(cwd).await?;
            let mut linked = None;
            for entry in registered.into_iter().skip(1) {
                if tokio::fs::canonicalize(&entry.directory)
                    .await
                    .ok()
                    .as_ref()
                    == Some(&requested)
                {
                    linked = Some(PathBuf::from(entry.directory));
                    break;
                }
            }
            let directory =
                linked.context("Only a registered linked worktree can be removed or reset")?;
            // Never destroy edits or untracked files.
            if !git(&directory, &["status", "--porcelain"])
                .await?
                .trim()
                .is_empty()
            {
                bail!("Worktree has uncommitted changes. Commit or move them before continuing.");
            }
            match operation {
                WorktreeOperation::Remove => {
                    git(
                        cwd,
                        &[
                            "worktree",
                            "remove",
                            "--",
                            directory.to_str().context("worktree path")?,
                        ],
                    )
                    .await?;
                }
                WorktreeOperation::Reset => {
                    let branch = devo_util_git::default_branch_name(cwd)
                        .await
                        .context("Repository default branch is unavailable")?;
                    git(&directory, &["reset", "--hard", &branch, "--"]).await?;
                }
                WorktreeOperation::List | WorktreeOperation::Create => {
                    unreachable!("mutation operation")
                }
            }
            Ok(serde_json::to_value(WorktreeMutationResult {
                directory: params.directory,
            })?)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[tokio::test]
    async fn plain_folders_have_no_worktrees_but_missing_folders_are_errors() -> Result<()> {
        let home = tempfile::tempdir()?;
        let folder = tempfile::tempdir()?;
        assert_eq!(
            execute(
                home.path(),
                json!({"cwd": folder.path()}),
                WorktreeOperation::List
            )
            .await?,
            json!({"worktrees": []})
        );
        assert!(
            execute(
                home.path(),
                json!({"cwd": folder.path().join("missing")}),
                WorktreeOperation::List
            )
            .await
            .is_err()
        );
        Ok(())
    }

    #[tokio::test]
    async fn lifecycle_lists_actual_worktrees_and_refuses_primary_or_dirty_checkouts() -> Result<()>
    {
        let home = tempfile::tempdir()?;
        let repo = tempfile::tempdir()?;
        git(repo.path(), &["init", "-b", "main"]).await?;
        git(
            repo.path(),
            &[
                "-c",
                "user.name=QA",
                "-c",
                "user.email=qa@example.invalid",
                "commit",
                "--allow-empty",
                "-m",
                "Initial",
            ],
        )
        .await?;
        let cwd = repo.path().to_string_lossy();
        assert_eq!(
            execute(home.path(), json!({"cwd": cwd}), WorktreeOperation::List).await?,
            json!({"worktrees": []})
        );
        let created = execute(
            home.path(),
            json!({"cwd": cwd, "name": "settings-qa"}),
            WorktreeOperation::Create,
        )
        .await?;
        let listed = execute(home.path(), json!({"cwd": cwd}), WorktreeOperation::List).await?;
        // Git prints native Windows paths with forward slashes.
        let expected = WorktreeInfo {
            directory: created["directory"].as_str().unwrap().replace('\\', "/"),
            name: created["name"].as_str().unwrap().to_string(),
            branch: created["branch"].as_str().unwrap().to_string(),
        };
        assert_eq!(listed, json!({"worktrees": [expected]}));
        let mutation = json!({"cwd": cwd, "directory": created["directory"]});
        execute(home.path(), mutation.clone(), WorktreeOperation::Reset).await?;
        let directory = Path::new(created["directory"].as_str().unwrap());
        tokio::fs::write(directory.join("keep.txt"), "Keep this file").await?;
        assert!(
            execute(home.path(), mutation.clone(), WorktreeOperation::Remove)
                .await
                .is_err()
        );
        assert!(
            execute(home.path(), mutation.clone(), WorktreeOperation::Reset)
                .await
                .is_err()
        );
        assert_eq!(
            tokio::fs::read_to_string(directory.join("keep.txt")).await?,
            "Keep this file"
        );
        tokio::fs::remove_file(directory.join("keep.txt")).await?;
        execute(home.path(), mutation, WorktreeOperation::Remove).await?;
        assert_eq!(
            execute(home.path(), json!({"cwd": cwd}), WorktreeOperation::List).await?,
            json!({"worktrees": []})
        );
        assert!(
            execute(
                home.path(),
                json!({"cwd": cwd, "directory": cwd}),
                WorktreeOperation::Remove
            )
            .await
            .is_err()
        );
        Ok(())
    }
}

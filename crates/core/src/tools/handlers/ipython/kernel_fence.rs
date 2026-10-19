use super::global_harness_state_dir;

/// Ensure a kernel exists for RLM sessions (created on the turn task, not the actor).
///
/// `session_dir` is the session **artifact** directory
/// (`session-artifacts/<id>/` for roots), not the parent of the rollout JSONL.
/// When provided, the kernel receives harness env so `rlm.harness` and
/// `await refine.run()` share the host's local store.
///
/// Build the kernel OS-fence spec on the `rlm-kernel` base profile (design doc
/// §5.1, P4): **no default read** — the kernel reads only the system/
/// interpreter base trees plus the roots granted here (workspace read-only in
/// Plan mode, read-write in Build; temp and session artifacts rw, global
/// harness rw, devo home rw). Reads and writes anywhere else are OS-refused,
/// so model code escalates through the rlm front door (`rlm.read`/`rlm.write`)
/// and the approval pipeline instead of the fence silently allowing it.
pub(crate) fn resolve_kernel_fence(
    cwd: &std::path::Path,
    session_dir: Option<&std::path::Path>,
    collaboration_mode: devo_protocol::CollaborationMode,
) -> Option<devo_kernel::KernelFenceSpec> {
    let mut writable_roots = Vec::new();
    if collaboration_mode == devo_protocol::CollaborationMode::Build {
        writable_roots.push(cwd.to_path_buf());
    }
    // Unix's base profile grants shared scratch roots too: retain them here
    // so the Plan overlap check sees every writable root in the actual fence.
    // Windows session kernels instead use owned scratch below the artifact
    // root, avoiding recursive ACL propagation across large shared temp trees.
    if session_dir.is_none() || !cfg!(windows) {
        writable_roots.extend(devo_sandbox::temp_writable_paths());
    }
    if let Some(dir) = session_dir {
        writable_roots.push(dir.to_path_buf());
    }
    // Global harness state is agent-owned memory/prompt/skill data — the same
    // trust class as session artifacts. The kernel harness API writes it
    // directly (`rlm.harness.create_memory(..., global_=True)` and friends),
    // and its own guidance points models there, so refusing it broke every
    // global create/update/delete with EACCES. Create the directory host-side
    // first: fence roots resolve to path-beneath rules against real
    // directories, and a missing root would downgrade the whole fence.
    let global_harness = global_harness_state_dir();
    if std::fs::create_dir_all(&global_harness).is_ok() && global_harness.is_dir() {
        writable_roots.push(global_harness);
    }
    // Devo home is agent-owned state and remains readable/writable in both modes.
    if let Ok(home) = devo_util_paths::find_devo_home()
        && std::fs::create_dir_all(&home).is_ok()
        && home.is_dir()
    {
        writable_roots.push(home);
    }
    Some(devo_kernel::KernelFenceSpec {
        readable_roots: if collaboration_mode == devo_protocol::CollaborationMode::Plan {
            vec![cwd.to_path_buf()]
        } else {
            Vec::new()
        },
        writable_roots,
        deny_read: Vec::new(),
        // RLM kernel fence: network is restricted by default (§5.2 — the fence
        // spec carries the RLM default, not the shell profile's; kernel network
        // goes through the proxy when granted, not through profile inheritance).
        restrict_network: true,
    })
}

#[cfg(test)]
mod tests {
    use super::resolve_kernel_fence;
    use pretty_assertions::assert_eq;

    #[test]
    fn devo_home_is_writable_in_plan_and_build() {
        let workspace = tempfile::tempdir().expect("workspace");
        let artifacts = tempfile::tempdir().expect("artifacts");
        let home = devo_util_paths::find_devo_home().expect("devo home");
        for mode in [
            devo_protocol::CollaborationMode::Plan,
            devo_protocol::CollaborationMode::Build,
        ] {
            let fence = resolve_kernel_fence(workspace.path(), Some(artifacts.path()), mode)
                .expect("kernel fence");
            assert_eq!(
                fence
                    .writable_roots
                    .iter()
                    .filter(|root| *root == &home)
                    .collect::<Vec<_>>(),
                vec![&home]
            );
        }
    }
}

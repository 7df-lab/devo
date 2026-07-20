//! Maps Devo [`WindowsSandboxRequest`] fields into sandbox permission types.

use crate::WindowsSandboxRequest;
use crate::protocol::models::PermissionProfile;
use crate::protocol::models::SandboxEnforcement;
use crate::protocol::permissions::FileSystemAccessMode;
use crate::protocol::permissions::FileSystemPath;
use crate::protocol::permissions::FileSystemSandboxEntry;
use crate::protocol::permissions::FileSystemSandboxPolicy;
use crate::protocol::permissions::NetworkSandboxPolicy;
use anyhow::Context;
use devo_util_paths::absolute_path::AbsolutePathBuf;
use std::path::Path;

pub(crate) fn permission_profile_from_request(
    req: &WindowsSandboxRequest,
) -> anyhow::Result<PermissionProfile> {
    let mut entries = Vec::new();
    for root in &req.readable_roots {
        entries.push(FileSystemSandboxEntry {
            path: FileSystemPath::Path {
                path: absolute_path(root)?,
            },
            access: FileSystemAccessMode::Read,
        });
    }
    for root in &req.writable_roots {
        entries.push(FileSystemSandboxEntry {
            path: FileSystemPath::Path {
                path: absolute_path(root)?,
            },
            access: FileSystemAccessMode::Write,
        });
    }
    for root in &req.deny_read {
        entries.push(FileSystemSandboxEntry {
            path: FileSystemPath::Path {
                path: absolute_path(root)?,
            },
            access: FileSystemAccessMode::Deny,
        });
    }

    let network = if req.restrict_network {
        NetworkSandboxPolicy::Restricted
    } else {
        NetworkSandboxPolicy::Enabled
    };

    Ok(
        PermissionProfile::from_runtime_permissions_with_enforcement(
            SandboxEnforcement::Managed,
            &FileSystemSandboxPolicy::restricted(entries),
            network,
        ),
    )
}

pub(crate) fn workspace_roots_from_request(
    req: &WindowsSandboxRequest,
) -> anyhow::Result<Vec<AbsolutePathBuf>> {
    Ok(vec![absolute_path(&req.cwd)?])
}

pub(crate) fn deny_read_overrides(
    req: &WindowsSandboxRequest,
) -> anyhow::Result<Vec<AbsolutePathBuf>> {
    req.deny_read
        .iter()
        .map(|path| absolute_path(path))
        .collect()
}

fn absolute_path(path: &Path) -> anyhow::Result<AbsolutePathBuf> {
    AbsolutePathBuf::from_absolute_path(path).or_else(|_| {
        dunce::canonicalize(path)
            .with_context(|| format!("failed to resolve absolute path {}", path.display()))
            .and_then(|resolved| AbsolutePathBuf::from_absolute_path(resolved).map_err(Into::into))
    })
}

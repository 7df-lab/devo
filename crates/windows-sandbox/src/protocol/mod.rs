//! Minimal permission/sandbox protocol types for the Windows sandbox.
//!
//! Full `devo-protocol` does not yet expose `PermissionProfile`; this module
//! holds only what the Windows sandbox crate needs.

pub mod config_types;
pub mod legacy_protocol;
pub mod models;
pub mod permissions;

// Re-exports are consumed by `devo_windows_sandbox::` callers and `cfg(windows)`
// modules; keep them even when the host (Darwin) lib build does not reference them.
#[allow(unused_imports)]
pub use config_types::WindowsSandboxLevel;
#[allow(unused_imports)]
pub use legacy_protocol::{NetworkAccess, SandboxPolicy, WritableRoot};
#[allow(unused_imports)]
pub use models::{ManagedFileSystemPermissions, PermissionProfile, SandboxEnforcement};
#[allow(unused_imports)]
pub use permissions::*;

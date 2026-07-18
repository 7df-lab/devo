//! Compiled permission rules and shell-aware access analysis.

mod bash_command_splitting;
mod file_access_model;
mod policy;
mod rules;
mod shell_access;
mod types;

pub use policy::CompiledPolicy;
pub use rules::PolicyCompileError;
pub use types::PermissionAccess;
pub use types::PolicyDecision;

use serde::{Deserialize, Serialize};

/// The mode controlling how the agent handles permission checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionMode {
    /// Approve every request without asking.
    AutoApprove,
    /// Ask the user for confirmation on each request.
    Interactive,
    /// Deny all requests that require permission.
    Deny,
}

/// What kind of resource a tool wants to access.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceKind {
    FileRead,
    FileWrite,
    ShellExec,
    Network,
    Custom(String),
}

/// A permission check request emitted by the tool system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub tool_name: String,
    pub resource: ResourceKind,
    /// Free-form description of what is being accessed.
    pub description: String,
    /// Optional path or command being accessed.
    pub target: Option<String>,
}

/// The result of a permission check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PermissionDecision {
    Allow,
    Deny { reason: String },
    Ask { message: String },
}

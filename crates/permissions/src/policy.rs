use async_trait::async_trait;

use crate::{PermissionDecision, PermissionRequest};

/// A pluggable policy that decides whether a tool action is allowed.
///
/// Different environments (CLI interactive, headless, SDK) can provide
/// different implementations.
#[async_trait]
pub trait PermissionPolicy: Send + Sync {
    async fn check(&self, request: &PermissionRequest) -> PermissionDecision;
}

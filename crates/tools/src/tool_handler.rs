use async_trait::async_trait;

use crate::contracts::{ToolCallError, ToolContext, ToolProgressSender, ToolResult};
use crate::tool_spec::ToolSpec;

/// The handler trait that every tool must implement.
///
/// Per L3-BEH-TOOLS-001, this trait uses `ToolContext` for execution context
/// and returns `ToolResult` (struct-based output) instead of trait objects.
#[async_trait]
pub trait ToolHandler: Send + Sync {
    /// Return the tool's specification.
    fn spec(&self) -> &ToolSpec;

    /// Execute the tool with the given context and input.
    async fn handle(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
        progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError>;
}

mod context;
mod orchestrator;
mod registry;
mod tool;
mod bash;
mod file_edit;
mod file_read;
mod file_write;
mod glob;
mod grep;

pub use bash::BashTool;
pub use context::*;
pub use file_edit::FileEditTool;
pub use file_read::FileReadTool;
pub use file_write::FileWriteTool;
pub use glob::GlobTool;
pub use grep::GrepTool;
pub use orchestrator::*;
pub use registry::*;
pub use tool::{Tool, ToolOutput, ToolProgressEvent};

use std::sync::Arc;

/// Register all built-in tools into a registry.
pub fn register_builtin_tools(registry: &mut ToolRegistry) {
    registry.register(Arc::new(BashTool));
    registry.register(Arc::new(FileReadTool));
    registry.register(Arc::new(FileWriteTool));
    registry.register(Arc::new(FileEditTool));
    registry.register(Arc::new(GlobTool));
    registry.register(Arc::new(GrepTool));
}

mod bash;
mod context;
mod file_edit;
mod file_read;
mod file_write;
mod glob;
mod grep;
mod orchestrator;
mod registry;
mod spec;
mod tool;

pub use bash::BashTool;
pub use context::*;
pub use file_edit::FileEditTool;
pub use file_read::FileReadTool;
pub use file_write::FileWriteTool;
pub use glob::GlobTool;
pub use grep::GrepTool;
pub use orchestrator::*;
pub use registry::*;
pub use spec::*;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_builtin_tools_populates_registry() {
        let mut registry = ToolRegistry::new();
        register_builtin_tools(&mut registry);

        let expected = [
            "bash",
            "file_read",
            "file_write",
            "file_edit",
            "glob",
            "grep",
        ];
        for name in &expected {
            assert!(
                registry.get(name).is_some(),
                "expected builtin tool '{}' to be registered",
                name
            );
        }
        assert_eq!(registry.all().len(), expected.len());
    }

    #[test]
    fn builtin_tools_have_nonempty_schemas() {
        let mut registry = ToolRegistry::new();
        register_builtin_tools(&mut registry);

        for tool in registry.all() {
            assert!(!tool.name().is_empty());
            assert!(!tool.description().is_empty());
            let schema = tool.input_schema();
            assert!(
                schema.is_object(),
                "tool '{}' schema should be an object",
                tool.name()
            );
        }
    }
}

use std::collections::HashMap;
use std::sync::Arc;

use crate::Tool;

/// Central registry of available tools.
///
/// The registry owns all tool instances and provides lookup by name.
/// Tools are registered once at startup and remain immutable for the
/// lifetime of the session.
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.get(name)
    }

    /// Return all tools for inclusion in the model request.
    pub fn all(&self) -> Vec<&Arc<dyn Tool>> {
        self.tools.values().collect()
    }

    /// Build tool definitions suitable for the model API.
    pub fn tool_definitions(&self) -> Vec<claw_provider::ToolDefinition> {
        self.tools
            .values()
            .map(|t| claw_provider::ToolDefinition {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.input_schema(),
            })
            .collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

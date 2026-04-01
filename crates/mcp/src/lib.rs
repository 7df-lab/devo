// MCP (Model Context Protocol) integration.
//
// This crate will provide:
// - MCP client connection management
// - Tool/resource/prompt discovery from MCP servers
// - Authentication and reconnection logic
// - Integration with the unified tool pool
//
// Implementation is planned for Phase 3. The types here are
// placeholders to reserve the crate's public API surface.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
}

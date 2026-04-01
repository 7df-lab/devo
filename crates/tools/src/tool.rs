use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::ToolContext;

/// The output returned by a tool after execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

impl ToolOutput {
    pub fn success(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            metadata: None,
        }
    }

    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            metadata: None,
        }
    }
}

/// Incremental progress events a tool can emit during execution.
///
/// These are surfaced to the UI layer (CLI spinner, TUI progress bar) without
/// blocking the tool's final result. Mirrors the `ToolProgressData` union in
/// Claude Code's `types/tools.ts`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolProgressEvent {
    /// Generic text status update (e.g. "compiling…", "fetching page 2/5").
    Status { message: String },
    /// Byte-level progress for long I/O operations.
    ByteProgress { done: u64, total: Option<u64> },
    /// A sub-command was spawned (tool_name, command string).
    SubCommand { tool: String, command: String },
}

/// The core trait every tool must implement.
///
/// Inspired by Claude Code's Tool.ts but redesigned for Rust:
/// - Tools receive only what they need via [`ToolContext`], not a giant context object.
/// - Schema is provided as JSON Schema for model compatibility.
/// - Read-only vs mutating is declared statically so the permission layer can optimize.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Unique tool name visible to the model.
    fn name(&self) -> &str;

    /// Human-readable description used in the model's tool prompt.
    fn description(&self) -> &str;

    /// JSON Schema describing the expected input.
    fn input_schema(&self) -> serde_json::Value;

    /// Execute the tool with validated input.
    async fn execute(
        &self,
        ctx: &ToolContext,
        input: serde_json::Value,
    ) -> anyhow::Result<ToolOutput>;

    /// Whether this tool only reads state (no side effects).
    fn is_read_only(&self) -> bool {
        false
    }

    /// Whether this tool can be run concurrently with others.
    fn supports_concurrency(&self) -> bool {
        self.is_read_only()
    }
}

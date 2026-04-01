use std::sync::Arc;

use tracing::{info, warn};

use claw_permissions::{PermissionDecision, PermissionRequest, ResourceKind};

use crate::{ToolContext, ToolOutput, ToolRegistry};

/// A pending tool call extracted from the model response.
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

/// The result of executing a single tool call.
#[derive(Debug, Clone)]
pub struct ToolCallResult {
    pub tool_use_id: String,
    pub output: ToolOutput,
}

/// Orchestrates the execution of tool calls.
///
/// Corresponds to Claude Code's `toolOrchestration.ts` and
/// `toolExecution.ts`. Handles:
/// - Looking up tools in the registry
/// - Permission checks before execution
/// - Serial vs concurrent dispatch
/// - Error wrapping
pub struct ToolOrchestrator {
    registry: Arc<ToolRegistry>,
}

impl ToolOrchestrator {
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self { registry }
    }

    /// Execute a batch of tool calls.
    ///
    /// Read-only tools that support concurrency are executed in parallel.
    /// Mutating tools are executed sequentially to avoid conflicts.
    pub async fn execute_batch(
        &self,
        calls: &[ToolCall],
        ctx: &ToolContext,
    ) -> Vec<ToolCallResult> {
        let mut results = Vec::with_capacity(calls.len());

        // Partition into concurrent (read-only) and sequential (mutating)
        let (concurrent, sequential): (Vec<_>, Vec<_>) = calls.iter().partition(|call| {
            self.registry
                .get(&call.name)
                .map(|t| t.supports_concurrency())
                .unwrap_or(false)
        });

        // Run concurrent tools in parallel
        if !concurrent.is_empty() {
            let futures: Vec<_> = concurrent
                .iter()
                .map(|call| self.execute_single(call, ctx))
                .collect();
            let concurrent_results = futures::future::join_all(futures).await;
            results.extend(concurrent_results);
        }

        // Run sequential tools one by one
        for call in &sequential {
            let result = self.execute_single(call, ctx).await;
            results.push(result);
        }

        results
    }

    async fn execute_single(&self, call: &ToolCall, ctx: &ToolContext) -> ToolCallResult {
        let Some(tool) = self.registry.get(&call.name) else {
            warn!(tool = %call.name, "tool not found");
            return ToolCallResult {
                tool_use_id: call.id.clone(),
                output: ToolOutput::error(format!("unknown tool: {}", call.name)),
            };
        };

        // Permission check for mutating tools
        if !tool.is_read_only() {
            let request = PermissionRequest {
                tool_name: call.name.clone(),
                resource: ResourceKind::Custom(call.name.clone()),
                description: format!("execute tool {}", call.name),
                target: None,
            };

            match ctx.permissions.check(&request).await {
                PermissionDecision::Allow => {}
                PermissionDecision::Deny { reason } => {
                    return ToolCallResult {
                        tool_use_id: call.id.clone(),
                        output: ToolOutput::error(format!("permission denied: {}", reason)),
                    };
                }
                PermissionDecision::Ask { message } => {
                    // Interactive approval is not yet wired to a UI prompt.
                    // Surface as a tool error so the model can report it to the user
                    // rather than silently failing. The CLI can later intercept this
                    // by providing a PermissionPolicy that blocks and asks the user.
                    return ToolCallResult {
                        tool_use_id: call.id.clone(),
                        output: ToolOutput::error(format!(
                            "permission required — run with --permission interactive to approve: {}",
                            message
                        )),
                    };
                }
            }
        }

        info!(tool = %call.name, id = %call.id, "executing tool");

        match tool.execute(ctx, call.input.clone()).await {
            Ok(output) => ToolCallResult {
                tool_use_id: call.id.clone(),
                output,
            },
            Err(e) => ToolCallResult {
                tool_use_id: call.id.clone(),
                output: ToolOutput::error(format!("tool execution failed: {}", e)),
            },
        }
    }
}

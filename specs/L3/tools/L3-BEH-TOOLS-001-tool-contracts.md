---
artifact_id: L3-BEH-TOOLS-001
revision: 2
status: Draft
active_baseline: no
---

# L3-BEH-TOOLS-001 — Tool Contracts

## Purpose

Define the `ToolHandler` trait, `ToolRegistry` struct, `ToolSpec`, `ToolContext`, and `ToolOutput`/`ToolError` types that form the contract between `core` (implementations) and `server` (consumers).

## Source Design

L2-DES-TOOL-001, L3-DES-ARCH-001

## 1. Dependency Contract

```
tools crate:
  depends on: protocol (for SessionId, TurnId, SessionMode, JsonSchema, RuntimePermissionProfile, CancellationToken)
  does NOT depend on: core, server, provider, safety

core crate:
  depends on: tools (implements ToolHandler, ToolRegistry)
  provides: ToolRegistryBuilder, all handler implementations

server crate:
  depends on: tools (consumes &dyn ToolRegistry, &dyn ToolHandler)
  calls: registry.get(name).handle(ctx, input, progress)
```

## 2. ToolSpec Validation Rules

```rust
impl ToolSpec {
    pub fn validate(&self) -> Result<(), Vec<SpecValidationError>> {
        let mut errors = Vec::new();
        if self.name.is_empty() || self.name.len() > 64 { errors.push("name: 1-64 chars"); }
        if !self.name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            { errors.push("name: lowercase alphanumeric + underscores only"); }
        if self.description.is_empty() || self.description.len() > 1000
            { errors.push("description: 1-1000 chars"); }
        if self.input_schema.type_name() != "object"
            { errors.push("input_schema: must be JSON object type"); }
    }
}
```

## 3. ToolProgressSender

```rust
pub struct ToolProgressSender {
    tx: mpsc::UnboundedSender<ToolProgress>,
}

pub enum ToolProgress {
    OutputDelta { content: String, stream_index: u32 },
    StatusUpdate { message: String },
    Completion { exit_code: Option<i32> },
}
```

## 5. What `tools` Must NOT Contain

- ❌ Any concrete tool implementation (no read, write, grep, shell, etc.)
- ❌ Any filesystem I/O
- ❌ Any process spawning
- ❌ Any network calls
- ❌ Any config reading
- ❌ Any permission checking logic
- ❌ Any approval logic
- ❌ Any JSONL serialization
- ❌ Any context assembly

## Traceability

| L2 Source | Relationship |
|---|---|
| L2-DES-TOOL-001 | specified-by |
| L3-DES-ARCH-001 | specified-by |

## Implementation Placement Guidance

- The tools crate contains pure contracts only: handler traits, tool specs, registry traits, errors, events, JSON schema helpers, handler kind, and summaries.
- Concrete handlers belong in core. Existing handler files in the tools crate are stale placement and should be migrated or replaced.
- `ToolRegistry` is a trait. Core provides the runtime implementation.
- `ToolContext` gains `tool_registry: Arc<dyn ToolRegistry>` for nested tool resolution (e.g., `multi_tool_use` needs to look up child tools).

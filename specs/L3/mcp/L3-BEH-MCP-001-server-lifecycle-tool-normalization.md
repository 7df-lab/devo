---
artifact_id: L3-BEH-MCP-001
revision: 1
status: Draft
active_baseline: no
---

# L3-BEH-MCP-001 — MCP Server Lifecycle and Tool Normalization

## Purpose

Define the concrete behavior for MCP server configuration, lifecycle management (startup, health, restart, stop), capability discovery, tool normalization into the program's registry, resource access, and error handling.

## Source Design

L2-DES-MCP-001 (MCP Integration Architecture)

## Behavior Specification

### B1. MCP Server Configuration Loading

- **Trigger**: Server starts or MCP configuration changes.
- **Preconditions**: `config.toml` may contain `[mcp.servers.<server_id>]` entries. `auth.json` may contain MCP credentials.
- **Algorithm / Flow**:
  1. Load each `McpServerRecord` from effective config:
     - `id` (McpServerId), `display_name`, `enabled`, `transport` (Stdio or StreamableHttp), `startup_policy` (Eager, Lazy, Manual), `trust_policy` (User, Workspace, Untrusted), `allowed_capabilities`, `roots_policy`, `output_limits`.
  2. For workspace-scoped servers with `trust_policy: Untrusted`: mark as `disabled` until user explicitly approves. Show warning in status.
  3. Resolve credential references from user-scoped `auth.json` for servers with `auth_ref`.
  4. Register each server in the `McpManager` with initial state `not_started` (eager) or `disabled` (if not enabled).
- **Postconditions**: All configured servers are registered with initial state.

### B2. Server Lifecycle State Machine

- **Trigger**: Server transitions based on startup policy, health checks, or user action.
- **Preconditions**: Server is registered in the MCP manager.
- **Algorithm / Flow**:
  State transitions:
  - `disabled` → `not_started` (when enabled)
  - `not_started` → `starting` (on eager startup, user start, or first use)
  - `starting` → `ready` (handshake and capability negotiation success)
  - `starting` → `failed` (handshake/transport/auth failure)
  - `starting` → `auth_required` (auth failure with recoverable credential prompt)
  - `ready` → `degraded` (health check failure, partial capability loss)
  - `ready`/`degraded`/`failed`/`auth_required` → `stopped` (on stop or disable)
  - `stopped` → `not_started` (on restart)
  For eager servers: start during runtime bootstrap. For lazy: start when first capability is needed. For manual: require explicit `mcp.startServer`.
- **Postconditions**: Each server has exactly one current state. State transitions are logged and broadcast.

### B3. Capability Discovery and Negotiation

- **Trigger**: Server reaches `starting` state and transport is established.
- **Preconditions**: Transport (stdio pipe or HTTP connection) is open.
- **Algorithm / Flow**:
  1. Send `initialize` request with client capabilities (tools, resources, resource_templates, prompts support).
  2. Receive server capabilities. Negotiate protocol version.
  3. Send `initialized` notification.
  4. Discover capabilities:
     a. `tools/list` → collect `{ name, description, inputSchema, annotations }`.
     b. `resources/list` → collect `{ uri, name, description, mimeType, size }`.
     c. `resources/templates/list` → collect `{ uriTemplate, name, description }`.
     d. `prompts/list` → collect `{ name, description, arguments }` if prompts enabled.
  5. Apply `allowed_capabilities` filter: only capabilities in the allowlist (if configured) are exposed.
  6. Transition to `ready`. Record `last_refresh` timestamp.
- **Postconditions**: MCP capabilities are normalized and available for tool registration.

### B4. Tool Normalization into Registry

- **Trigger**: Capability discovery completes successfully.
- **Preconditions**: MCP tools were discovered. The tool registry is initialized.
- **Algorithm / Flow**:
  1. For each discovered MCP tool, create a normalized `ToolDefinition`:
     - `tool_name`: `mcp.<server_id>.<tool_name>` (collision-free namespace).
     - `display_name`: `[<server_display>] <tool_name>`.
     - `server_id`, `mcp_tool_name` (original MCP name).
     - `description`: MCP-provided description (untrusted, for display only).
     - `input_schema`: MCP-provided schema.
     - `execution_mode`: inferred from annotations or default `external_side_effect`.
     - `tool_category`: `External`.
     - `permission_policy`, `redaction_policy`, `output_limit_policy`: from config defaults + server-specific overrides.
     - Handler: MCP tool invocation handler (dispatches `tools/call` to originating server).
  2. Register each normalized tool in the tool registry.
  3. If an MCP server disconnects or tools change after refresh: update registry (add new, remove stale).
- **Postconditions**: MCP tools are available to the model through the normal tool registry and lifecycle.

### B5. MCP Tool Invocation

- **Trigger**: Model requests a normalized MCP tool via the tool supervisor.
- **Preconditions**: The tool is registered. The originating server is in `ready` or `degraded` state.
- **Algorithm / Flow**:
  1. The tool passes normal validation, permission, approval, and safety gates (L3-BEH-TOOLS-002).
  2. Before execution: check server availability. If server is not `ready` → return `unavailable` result.
  3. Send `tools/call` to the originating server: `{ name: <mcp_tool_name>, arguments: <args> }`.
  4. Receive response: structured content (text, image, resource refs) and `isError` flag.
  5. Normalize output: extract text content, map resource refs, set `is_error` from `isError`.
  6. Apply output bounding and redaction per tool policy.
  7. Return structured result with `result_summary`, `structured_status`, and `content`.
- **Postconditions**: MCP tool result follows the same lifecycle as built-in tools. Server attribution is preserved.
- **Error Handling**: Server unavailable → `unavailable` result. Tool disappeared → `unavailable`, remove from registry. Timeout → `failed` with timeout error.

### B6. Resource Access

- **Trigger**: Client sends `mcp.readResource` or model invokes a resource-read tool.
- **Preconditions**: The MCP server is capable of resource access. The resource URI is valid.
- **Algorithm / Flow**:
  1. Validate the resource URI against known resources or resolve via resource template.
  2. For templates: validate URI parameters. Expand template to concrete URI.
  3. Send `resources/read` with `{ uri }` to the server.
  4. Apply size limits: if `size` exceeds `output_limit_policy`, truncate with notice.
  5. Apply MIME-type filtering: block binary types by default unless explicitly configured.
  6. Apply redaction before injecting into context or displaying to client.
  7. Resource content is not automatically injected into model context — only when explicitly requested.
- **Postconditions**: Resource content is safely retrieved and bounded.

### B7. Error Handling and Client Visibility

- **Trigger**: MCP errors occur at any lifecycle stage.
- **Preconditions**: The error is caught and classified.
- **Algorithm / Flow**:
  1. Classify errors: `ConfigurationInvalid`, `ServerDisabled`, `ServerNotTrusted`, `StartupFailed`, `TransportFailed`, `ProtocolNegotiationFailed`, `AuthenticationRequired`, `CapabilityUnavailable`, `InputInvalid`, `ToolInvocationFailed`, `ResourceReadFailed`, `OutputRejectedByPolicy`, `OperationCanceled`.
  2. Each error carries: `server_id`, `error_code`, `message` (safe for user), `recovery_context`.
  3. Broadcast MCP status changes via `server/status/changed` event with MCP-specific payload.
  4. Client projections (via `mcp.listServers`): server id, display name, enabled state, lifecycle state, auth state, last refresh, safe error summary, capability counts, disabled reasons.
- **Postconditions**: Users can see MCP server status and understand failures.

## Traceability

| L2 Source | Relationship |
|---|---|
| L2-DES-MCP-001 | specified-by |

## Implementation Placement Guidance

- `McpManager` is the runtime owner for configured MCP server connections. Its crate placement may be `crates/mcp` or core-adjacent, but lifecycle ownership must match this L3 contract.
- Each MCP server connection uses the `mcp-client` crate (or a custom stdio/HTTP MCP client) for protocol communication.
- Tool namespacing `mcp.<server_id>.<tool_name>` prevents collisions between servers.
- Stale tools: after each capability refresh, compare old and new tool lists. Remove tools from registry that no longer exist. Add new tools.

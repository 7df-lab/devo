---
artifact_id: L3-DES-ARCH-001
revision: 4
status: Draft
active_baseline: no
---

# L3-DES-ARCH-001 — Crate Architecture

## Purpose

Define the final crate responsibility boundaries, inter-crate contracts, data ownership rules, and L2 design mapping needed to implement the L2 architecture without leaking stale implementation constraints into new development.

## 1. Architecture Principles

The `crates/` directory may be used as reference material only. This document is normative for ownership boundaries even when current modules, filenames, file counts, or type locations differ.

### Normative Rules

1. **Pure DTO boundary**: `protocol` owns serializable request, response, event, id, and projection DTOs. It does not own replay, permission, approval, context, model, or tool execution behavior.
2. **Contract boundary**: `tools` owns the tool contracts (`ToolHandler`, `ToolSpec`, `ToolRegistry`, tool errors, progress types). It does not own concrete handlers.
3. **Domain boundary**: `core` owns business decisions, durable data models, replay, configuration resolution, context assembly, built-in tool handlers, permission/approval decisions, model binding resolution, memory, skill activation policy, workspace discovery, and restore/fork/goal policies.
4. **Skill package boundary**: `skills` owns skill package definitions, `SKILL.md` parsing, default bundled skill manifests, and idempotent default-skill installation. It does not decide when a skill is activated for a session.
5. **Runtime boundary**: `server` owns client/server runtime concerns: JSON-RPC transport, connection registry, active turn slots, event sequencing/broadcast, server lifecycle, and orchestration of core/provider/tool calls.
6. **External adapter boundary**: `provider` owns provider HTTP/API protocol adapters and normalized provider stream events. `mcp` owns or hosts MCP transport/lifecycle behavior and exposes normalized capabilities to the normal tool registry.
7. **Enforcement boundary**: `safety` owns mechanical sandbox enforcement at process/network/filesystem boundaries. It does not decide whether an operation is allowed; core decides policy and safety enforces the resulting constraints.
8. **UI boundary**: `tui`, desktop, IDE, and future clients consume protocol/client projections. They do not execute tools, resolve permissions, mutate JSONL directly, or infer authoritative session state from local UI state.
9. **Projection boundary**: SQLite or other indexed stores are derived projections unless an L3 explicitly says otherwise. Append-only JSONL session records remain the replay authority for sessions.

### Effect Ownership

The architecture separates **who decides** from **who performs I/O**:

- Core decides whether local side effects are allowed and records their durable consequences.
- Built-in core handlers may perform local workspace/file/process effects when required by their L3 contracts.
- Safety applies sandbox constraints to process, filesystem, and network boundaries.
- Provider adapters perform model-provider HTTP/API I/O and return normalized stream events.
- Server owns transport I/O with clients and orchestration timers, not domain decisions.
- Clients render and submit requests; they do not perform authoritative agent side effects.

## 2. Final Crate Architecture

```
Dependency arrows point from a crate to crates it may depend on.

cli ───────► server, tui, client, core
tui ───────► client, protocol, file-search
client ────► protocol
server ────► core, provider, protocol
core ──────► protocol, tools, safety, utils, tasks, file-search, mcp, skills
mcp ───────► protocol, tools
skills ────► none preferred
provider ─► protocol
tools ─────► protocol
safety ────► protocol
tasks ─────► protocol
utils ─────► none preferred
file-search ► none preferred
```

### Crate Definitions

#### `protocol` — Shared Data Structures
- **Responsibility**: All types exchanged between crates. JSON-RPC envelopes, session/turn/item/event types, error codes.
- **Exposes**: `SessionId`, `TurnId`, `ItemId`, `TurnMetadata`, `InputItem`, `ContentBlock`, `Message`, `Role`, `ProtocolErrorCode`, `ClientRequest`, `ServerNotification`, event payload structs, `ToolDefinition` projection.
- **Depends on**: Nothing (zero deps beyond serde, chrono, uuid).
- **Must NOT contain**: Any business logic, validation beyond serde deserialization, I/O.

#### `core` — Business Logic (Heavy Crate)
- **Responsibility**: All domain logic. Data model serialization, config resolution, context assembly/compaction/normalization, tool handler implementations, tool registry construction, permission evaluation, approval decision pipeline, model binding resolution, persistence triggering, memory pipeline, skill catalog resolution and activation policy, workspace discovery, fuzzy search provider trait.
- **Exposes**:
  - **Data model**: `SessionRecord`, `TurnRecord`, `ItemRecord`, `PlanRecord`, `GoalRecord`, `FileChange`, `Mention`, `ContentPart`, all JSONL record types and replay engine.
  - **Config**: `ConfigLayers`, `EffectiveConfig`, config resolution/merge.
  - **Context**: `ContextAssembler` (trait), `CompactionEngine`, `ContextNormalizer`.
  - **Tools**: All built-in handler implementations (`ReadHandler`, `WriteHandler`, `ApplyPatchHandler`, `GrepHandler`, `GlobHandler`, `LsHandler`, `ShellHandler`, `WebSearchHandler`, `FetchUrlHandler`, `PlanHandler`, `GoalUpdateHandler`, `ApprovalHandler`, `QuestionHandler`, subagent handlers, `MultiToolUseHandler`, `ToolSearchHandler`), `ToolRegistryBuilder`.
  - **Permissions**: `PermissionProfile`, `AccessMode`, `resolve_access()`, `can_read()`, `can_write()`, `network_enabled()`, materialization of symbolic paths.
  - **Approval**: `authorize_tool_request()` entry point, `ApprovalCache`, `ApprovalDecision`, auto-reviewer logic, circuit breaker.
  - **Model**: `SupportedModelDefinition` catalog, `ModelProviderBinding` validation, `ResolvedModelProfile` construction.
  - **Memory**: Memory extraction (Phase 1), consolidation (Phase 2), read path, job concurrency.
  - **Skills**: runtime `SkillCatalog`, discovery-root resolution, activation policy, trust checks, active skill session state, context integration. Package parsing and default installation come from the `skills` crate.
  - **Workspace**: Project root detection, instruction file discovery.
  - **Search**: `SearchProvider` trait, `FileSearchProvider` (uses `file-search` crate).
  - **Persistence**: `SessionStore` trait (JSONL append/replay), persistence triggers.
- **Depends on**: `protocol`, `tools`, `safety`, `utils`, `tasks`, `file-search`, `mcp`, and `skills` as needed. Does NOT depend on `server`, `provider`, `tui`, or `client`.
- **May contain**: Local workspace/filesystem I/O and process execution required by built-in tool handlers, but only through permission, approval, sandbox, cancellation, output bounding, redaction, and durable record contracts.
- **Must NOT contain**: Client transport, provider HTTP/API adapters, terminal rendering, desktop/IDE UI behavior, JSON-RPC connection management, or client subscription state.

#### `server` — Orchestration (Light Crate)
- **Responsibility**: Wraps core in a runtime. Transport, turn orchestration, event broadcast, connection management, interrupt propagation, active work reservation, and server lifecycle.
- **Exposes**: `ServerRuntime` (startup/shutdown), `Transport` (trait), `ClientRegistry`, `EventBroadcaster`.
- **Depends on**: `core`, `protocol`, and `provider` for model invocation adapters.
- **Turn orchestration**: Admits turns, asks core to assemble or update state, invokes provider adapters when core has prepared a provider request, dispatches tool calls through the core-built registry, and broadcasts normalized events to clients.
- **Must NOT contain**: Tool implementation logic, config merge logic, permission/approval decision logic, context selection logic, JSONL schema decisions, memory consolidation policy, or TUI rendering behavior.

#### `tools` — Tool Contracts (Light Crate)
- **Responsibility**: Only `ToolHandler` trait, `ToolRegistry` lookup interface, `ToolSpec` schema definition.
- **Exposes**:
  - `trait ToolHandler`: `async fn handle(&self, ctx: ToolContext, input: Value) -> Result<ToolOutput, ToolError>`
  - `ToolSpec { name, display_name, description, input_schema, output_mode, execution_mode, capability_tags, supports_parallel, supports_cancellation, supports_streaming, preparation_feedback }`
  - `ToolRegistry`: `fn get(&self, name: &str) -> Option<&Arc<dyn ToolHandler>>`, `fn spec(&self, name: &str) -> Option<&ToolSpec>`, `fn list_available(&self, mode: &SessionMode, permission: &PermissionProfile) -> Vec<&ToolSpec>`
  - `ToolContext { session_id, turn_id, tool_call_id, workspace_root, permission_profile, tool_registry, output_limit_bytes, cancel_token }`
  - `ToolOutput { content, display_content, structured_status, result_summary, redaction_state, safety_notice }`
  - `ToolError { code, message, recoverable }`
- **Depends on**: `protocol` (for `ToolDefinition` projection). Does NOT depend on `core`, `server`, `provider`.
- **Must NOT contain**: Any concrete tool implementation, any I/O, any permission checking, any config reading, any approval logic, or any durable persistence logic.

#### `skills` — Skill Package Definitions and Defaults
- **Responsibility**: Skill package data model, `SKILL.md` frontmatter/body parsing, package validation diagnostics, supporting-resource path resolution helpers, bundled default skill manifests, and idempotent installation of default skills into the user skill root.
- **Exposes**:
  - `SkillDefinition`, `SkillMetadata`, `SkillPackage`, `SkillResourceRef`, `SkillSourceKind`, `SkillDiagnostic`.
  - `SkillPackageParser` for validating `SKILL.md` plus package layout.
  - `DefaultSkillBundle` for read-only shipped skill packages.
  - `DefaultSkillInstaller` for first-run and version-update installation of default skills.
- **Depends on**: Prefer no domain crate dependencies. It may use general filesystem/archive parsing libraries, but it must not depend on `core`, `server`, `tui`, `client`, `provider`, or `tools`.
- **Must NOT contain**: Session activation decisions, instruction precedence, permission decisions, context assembly, tool execution, or client/server protocol handling.

#### `safety` — Sandbox Enforcement (Independent)
- **Responsibility**: Sandbox policy enforcement at the OS boundary. Process isolation constraints, filesystem jail, network egress filtering.
- **Exposes**: `SandboxPolicy`, `apply_sandbox(command: Command) -> SandboxedCommand`, `NetworkEgressFilter`.
- **Depends on**: `protocol` (for error types). Does NOT depend on `core`.
- **Note**: Permission types (`PermissionProfile`, `AccessMode`, permission evaluation) live in `core`, not here. Safety only enforces sandbox — the mechanical "can this process touch this path/port."

#### `provider` — Provider Adapters (Independent)
- **Responsibility**: Provider HTTP/API protocol adapters. Request serialization, response streaming, SSE/WebSocket/HTTP response parsing when applicable, provider event normalization, provider-specific usage extraction.
- **Exposes**: `ProviderRouter` (server-facing facade), `ModelProviderSDK` or equivalent adapter traits (provider-internal/provider-family-facing), `ProviderRequest`, `ProviderEvent`, `StreamNormalizer`.
- **Depends on**: `protocol` (for event types). Does NOT depend on `core`, `server`.

#### `tui` — Terminal UI (Independent)
- **Responsibility**: All terminal rendering and user interaction. Ratatui-based layout, composer, transcript, streaming cells, approval modals, slash commands, full transcript overlay, and onboarding UI when launched by CLI onboarding mode.
- **Depends on**: `client` (for server connection), `protocol` (for event types), `file-search` (for @-mention search).
- **Must NOT contain**: Business logic, tool execution, permission decisions, config resolution.

#### `client` — Server SDK (Independent)
- **Responsibility**: WebSocket connection management, JSON-RPC serialization, reconnection, event subscription, `Client` struct for TUI/IDE/desktop consumers.
- **Depends on**: `protocol`.

#### `cli` — Entry Point (Thin)
- **Responsibility**: Argument parsing (clap), server lifecycle management (fork/exec), onboarding trigger, signal handling.
- **Depends on**: `server`, `tui`, `client`, `core` (for config loading).

#### `mcp` — MCP Types (Thin)
- **Responsibility**: MCP protocol types and MCP server lifecycle/transport support when implemented as a separate crate: `McpServerId`, `McpServerRecord`, `McpTransportConfig`, `McpAuthConfig`, capability discovery, health state, and normalized MCP call transport.
- **Integration**: MCP capabilities become normal tool registry entries through core-owned registry construction and permission/approval policy. MCP transport code does not bypass core tool lifecycle rules.
- **Depends on**: `protocol` and `tools` for normalized capability contracts. Does NOT depend on `tui` or `client`.

#### `file-search` — Fuzzy File Search (Utility)
- **Responsibility**: High-performance incremental file search using `nucleo` + `ignore` walker. Used by `core::search` and `tui` @-mention.
- **Depends on**: Nothing beyond nucleo, ignore crates.

#### `utils` — General Utilities
- **Responsibility**: ANSI escape processing, fuzzy string matching, git operations, config path resolution, terminal detection, shell command parsing.
- **Depends on**: Prefer no domain crate dependencies. A dependency on `protocol` is allowed only for pure DTO helpers that do not pull in domain behavior.

#### `tasks` — Job/Task Primitives
- **Responsibility**: Background job tracking primitives, lease-based job coordination.
- **Depends on**: `protocol`.

#### `arg0` — CLI Preprocessing
- **Responsibility**: Argument forwarding for multi-call binary patterns.
- **Depends on**: `core`, `server`, `utils`.

## 3. Inter-Crate Contracts

### core → server

```rust
// In core crate — the main entry point server calls per turn
pub async fn query(
    session: &SessionRecord,
    turn: &TurnRecord,
    context: &AssembledContext,
    model: &ResolvedModelProfile,
    tool_registry: &dyn ToolRegistry,
    permission_profile: &RuntimePermissionProfile,
    approval_cache: &ApprovalCache,
) -> Result<QueryOutcome, QueryError>;

pub enum QueryOutcome {
    TerminalResponse {
        item: ResponseItem,
        usage: TurnUsage,
    },
    ToolCallsRequired {
        tool_calls: Vec<ToolCallRequest>,
        usage_so_far: TurnUsage,
    },
}

// Server calls this to execute a tool after approval gates pass
pub async fn execute_tool(
    tool_name: &str,
    input: serde_json::Value,
    ctx: ToolContext,
) -> Result<ToolOutput, ToolError>;

// Server calls this for permission decisions
pub fn authorize_tool_request(
    tool_name: &str,
    tool_category: ToolCategory,
    resource: &ResourceKind,
    profile: &RuntimePermissionProfile,
    cache: &mut ApprovalCache,
    policy: &ApprovalPolicy,
) -> PermissionDecision;
```

### tools → core (trait impl direction)

```rust
// Defined in tools crate, IMPLEMENTED in core crate
#[async_trait]
pub trait ToolHandler: Send + Sync {
    fn spec(&self) -> &ToolSpec;
    async fn handle(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
        progress: Option<ToolProgressSender>,
    ) -> Result<ToolOutput, ToolError>;
}

// Defined in tools crate, POPULATED in core crate
pub trait ToolRegistry: Send + Sync {
    fn get(&self, name: &str) -> Option<&Arc<dyn ToolHandler>>;
    fn spec(&self, name: &str) -> Option<&ToolSpec>;
    fn list_available(&self, session_mode: &SessionMode, permission: &PermissionProfile) -> Vec<&ToolSpec>;
    fn list_all_specs(&self) -> &[ToolSpec];
}
```

### provider → server

```rust
// Defined in provider crate as the server-facing facade
#[async_trait]
pub trait ProviderRouter: Send + Sync {
    async fn stream(
        &self,
        model: ResolvedModelProfile,
        input: ProviderInvocationInput,
        cancel_token: CancellationToken,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError>;
}
```

Provider adapters are invoked by server orchestration after core has resolved a `ResolvedModelProfile` and prepared provider-neutral `ProviderInvocationInput`. The provider crate serializes that input into provider-specific request bodies, selects the invocation-method adapter, and emits normalized `ProviderEvent` values. Core consumes those events for durable records, context continuation, and usage accounting.

Provider-family-specific adapter traits such as `ModelProviderSDK` may exist behind `ProviderRouter`, but server runtime should depend on the router facade rather than on individual provider SDK implementations.

### safety → core tool handlers

```rust
// Defined in safety crate
pub trait Sandbox: Send + Sync {
    fn constrain_command(&self, command: &mut std::process::Command, profile: &SandboxPolicy);
    fn constrain_network(&self, target: &url::Url, policy: &NetworkEgressFilter) -> bool;
}
```

Core permission/approval logic decides which `SandboxPolicy` applies. Safety implementations apply that policy at the OS boundary and return enforcement diagnostics; they do not decide user intent or approval status.

### Data Flow Direction

```
User Input (tui)
  → client (JSON-RPC)
  → server (turn/submit handler)
  → core admits turn and appends durable input records
  → core assembles/normalizes context and prepares provider request
  → server invokes provider adapter and receives normalized provider events
  → core consumes provider events into durable records, tool requests, usage, and continuation context
  → if tool calls: core validates and authorizes them
  → server schedules admitted tool calls through the core-built registry
  → core handlers perform bounded effects and produce durable/client event intents
  → server sequences and broadcasts events via client connections
  → tui renders events
```

## 4. L2 Design Mapping

| Crate | L2 Designs |
|---|---|
| `protocol` | APP-003 (JSON-RPC envelopes, event payloads), CONV-001 (ID types) |
| `core` | CONV-001 (full data model, JSONL records, replay), CONTEXT-001/002/003 (assembly, compaction, normalization), TOOL-001 (handler implementations, registry build), TOOL-002 (multi_tool_use), TOOL-003 (deferred loading), SAFETY-001 (permission evaluation, profile resolution), SAFETY-002 (approval pipeline, auto-reviewer, cache), MODEL-001 (binding resolution), MEM-001 (memory pipeline), SKILLS-001 (runtime catalog resolution and activation), WORKSPACE-001 (instruction discovery), APP-002 (persistence triggers), APP-005 (config schema), APP-006 (search provider trait), GOAL-001 (goal state machine) |
| `server` | AGENT-001 (turn execution loop orchestration), AGENT-002 (interrupt propagation, resume), AGENT-003 (subagent session management), APP-003 (transport, broadcast, sequence), APP-001 (process ownership) |
| `tools` | TOOL-001 (handler contract, spec, registry interface) |
| `skills` | SKILLS-001 (skill package definition, parser, bundled default skills, default-skill installation) |
| `safety` | SAFETY-001 (sandbox enforcement at OS boundary) |
| `provider` | MODEL-001 (invocation method adapters), LLM-003 (stream event normalization) |
| `tui` | TUI-001 through TUI-010 (all TUI L2s), TUI-CMD-001 through TUI-CMD-012 (slash commands), CLIENT-001/002/003 (rendering behavior) |
| `client` | APP-003 (WebSocket transport, reconnection) |
| `cli` | APP-007 (entry point, onboarding trigger) |
| `mcp` | MCP-001 (server lifecycle, capability discovery, normalized tool transport) |
| `file-search` | APP-006 (fuzzy search backend) |
| `tasks` | MEM-001 (job coordination primitives) |

## 5. Acceptance Criteria Self-Check

| Criterion | Status |
|---|---|
| Each crate has clear, unambiguous responsibility boundaries | ✓ |
| No circular dependencies | ✓ (server depends on core/provider/protocol; core depends on contracts/adapters but not server/client/tui/provider) |
| `protocol` and `tools` remain pure data/contract crates | ✓ |
| `skills` owns package/default installation mechanics without owning session activation policy | ✓ |
| `core` owns domain decisions and permitted local tool effects, but not provider/client/UI transport | ✓ |
| `server` holds runtime orchestration and event broadcast but no UI rendering | ✓ |
| `tools` only defines handler contract and registry, not implementations | ✓ |
| `safety`/`provider`/`tui` are independent replaceable modules | ✓ |

**Note on core/protocol/tools**: The "no business logic" constraint applies to `protocol` and the pure contract parts of `tools`. `core` is intentionally the domain-heavy crate. It may perform local effects only where L3 tool-handler behavior requires them and only through permission, approval, sandbox, output bounding, redaction, and durable record contracts. It must not absorb provider HTTP adapters, client transport, or UI rendering.

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---|---:|---|---|---|
| 1 | 2026-05-27 | Assistant | Initial | Crate architecture evaluation, final layout, inter-crate contracts, L2 mapping. |
| 2 | 2026-05-27 | Assistant | Correction | Removed stale current-crate evaluation, clarified normative ownership boundaries, corrected dependency direction, and separated core local effects from server/provider/client transport I/O. |
| 3 | 2026-05-27 | Assistant | Correction | Added dedicated `skills` crate for skill package definitions, parsing, bundled defaults, and default-skill installation; narrowed core to runtime catalog and activation policy. |
| 4 | 2026-05-27 | Assistant | Correction | Added a server-facing `ProviderRouter` facade so server invokes provider transport without depending on provider-family SDK traits directly. |

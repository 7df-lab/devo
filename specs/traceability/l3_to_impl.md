# L3 to Implementation Traceability Matrix

| L3 Spec ID | Implementation Location | Status | Notes |
|---|---|---|---|
| L3-BEH-CORE-001 | `durable_record.rs`, `session_store.rs`, `jsonl_store.rs`, `replay.rs` | Complete | DurableRecord (45 variants), SessionStore trait, JsonlSessionStore, ReplayProjection |
| L3-BEH-CORE-002 | `execution.rs` | Complete | admit_turn, prepare_model_invocation, consume_provider_event, finish_model_invocation |
| L3-BEH-CORE-003 | `crates/tools/src/` | Partial | Handlers in tools crate; spec requires migration to core |
| L3-BEH-CORE-004 | `permission.rs` | Complete | authorize_tool_request, ApprovalCache, AutoReviewerState |
| L3-BEH-CORE-005 | `context_pipeline.rs` | Complete | ContextAssembler, CompactionEngine, ContextNormalizer |
| L3-BEH-CORE-006 | `durable_record.rs` | Complete | ContentPart, Mention, WorkspaceChangeSet, FileChange, Plan/Goal types |
| L3-BEH-CORE-007 | `memory.rs` | Partial | MemoryWorkspace, MemoryStore trait, Stage1Output; model-dependent |
| L3-BEH-CORE-008 | `instruction_discovery.rs` | Complete | discover_project_root, discover_instructions |
| L3-BEH-CORE-009 | `context_pipeline.rs` | Complete | 3-pass normalization |
| L3-BEH-CORE-010 | `fuzzy_search.rs` | Partial | SearchProvider trait; nucleo impl pending |
| L3-BEH-CORE-011 | `fork.rs`, `durable_record.rs` | Partial | validate_fork_request, create_fork_session |
| L3-BEH-CORE-012 | `message_edit.rs` | Complete | check_edit_eligibility, create_edit_records, restore planning |
| L3-BEH-PROTOCOL-001 | `protocol/src/session.rs` | Partial | SessionSubscribe, SessionDelete, MessageEditPrevious DTOs |
| L3-BEH-PROVIDER-001 | `model_binding.rs`, `provider/src/error.rs` | Complete | Model types, ProviderError (11 variants) |
| L3-BEH-PROVIDER-002 | `provider/src/` | Partial | SSE parsing works; coalescence pending |
| L3-BEH-PROVIDER-003 | `durable_record.rs` | Partial | UsageMetric, ContextPressure types |
| L3-BEH-TOOLS-001 | `tools/src/contracts.rs` | Complete | ToolContext, ToolResult, ToolCallError, ToolRegistry trait |
| L3-BEH-TOOLS-002 | `tools/src/` | Partial | TerminalStatus types; validation pending |
| L3-BEH-TOOLS-003 | `tools/src/router.rs` | Partial | Concurrent execution; multi_tool_use pending |
| L3-BEH-TOOLS-004 | `tools/src/deferred_loading.rs`, `tools/src/handlers/tool_search.rs`, `tools/src/registry.rs`, `core/src/tool_prompt.rs` | Partial | Classification, callable ToolSearch handler, alias resolution, session loaded set, prompt reminder, metrics, and core request-surface assembly; durable session metadata and live query integration pending |
| L3-BEH-MCP-001 | `mcp/src/lib.rs`, `mcp/src/manager.rs` | Partial | InMemoryMcpManager, 8 lifecycle states |
| L3-BEH-SERVER-001 | `server/src/runtime.rs`, `transport.rs` | Partial | ServerRuntime; EventBroadcaster missing |
| L3-BEH-SERVER-002 | `server/src/runtime/handlers/turn.rs` | Partial | Basic interrupt; durable records pending |
| L3-BEH-SERVER-003 | `server/src/subagent.rs` | Partial | AgentRegistry, Mailbox types; handlers pending |
| L3-BEH-SERVER-004 | `server/src/goal.rs`, `handlers/goal.rs` | Complete | Goal lifecycle, GoalStore, state machine |
| L3-BEH-SERVER-005 | `server/src/runtime/skills.rs`, `core/src/skills.rs` | Partial | Discovery; activation flow pending |
| L3-BEH-SAFETY-001 | `safety/src/sandbox.rs` | Partial | Sandbox trait; platform impls pending |
| L3-BEH-SAFETY-002 | `core/src/permission.rs` | Partial | Pipeline in core; ExecutionGrant pending |
| L3-BEH-SKILLS-001 | `skills/src/` | Complete | SkillPackage, parser, DefaultSkillInstaller |
| L3-BEH-CLI-001 | `cli/src/main.rs` | Partial | Subcommand-based; flag-based args pending |
| L3-BEH-CLIENT-001 | `client/src/stdio.rs` | Partial | Stdio; WebSocket missing |
| L3-BEH-TUI-001..008 | `tui/src/` | Partial | Layout, streaming, /goal added |
| L3-BEH-APP-001 | `config_resolution.rs` | Partial | Types; atomic write pending |
| L3-BEH-APP-002 | `logging.rs` | Partial | Basic logging; diagnostics pending |
| L3-BEH-APP-003 | `specs/l1_l2_traceability_gaps.py`, `specs/l2_l3_traceability_gaps.py`, `specs/traceability/` | Partial | L1-L2 and L2-L3 validators exist; L3 implementation and verification validators pending |
| L3-DES-ARCH-001 | `crates/` | Partial | 14 crates; skills created; dep violations remain |

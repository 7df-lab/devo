# Verification Matrix — Tests to Spec Coverage

| Spec ID | Test Location | Test Name | Verifies |
|---|---|---|---|
| L3-BEH-CORE-001 | `crates/core/src/durable_record.rs` | `session_record_serde_roundtrip`, `turn_record_serde_roundtrip`, `rollout_line_all_variants_roundtrip`, etc. | DurableRecord serialization, turn state machine |
| L3-BEH-CORE-001 | `crates/core/src/jsonl_store.rs` | `append_and_replay_roundtrip`, `replay_truncated_file_handles_partial_line`, `concurrent_appends_to_different_sessions` | SessionStore append/replay/flush/file_size |
| L3-BEH-CORE-001 | `crates/core/src/replay.rs` | `empty_records_produces_empty_projection`, `turn_lifecycle_replays`, `unterminated_turn_marked_interrupted` | ReplayProjection builder |
| L3-BEH-CORE-002 | `crates/core/src/execution.rs` | `admission_to_context_assembly_is_legal`, `all_legal_transitions_are_valid`, `terminal_cannot_transition` | TurnExecutionPhase state machine |
| L3-BEH-CORE-002 | `crates/core/src/execution.rs` | `turn_runtime_state_starts_in_admission`, `drain_clears_buffers` | TurnRuntimeState |
| L3-BEH-CORE-002 | `crates/core/src/execution.rs` | `outcome_terminal_response`, `outcome_tool_calls_required` | ModelInvocationOutcome |
| L3-BEH-CORE-004 | `crates/core/src/permission.rs` | `resolve_read_only_profile`, `resolve_access_longest_prefix_wins`, `auto_approve_mode_allows_everything`, `default_mode_asks_for_shell_exec` | Permission profile resolution, access evaluation, authorization pipeline |
| L3-BEH-CORE-004 | `crates/core/src/permission.rs` | `auto_reviewer_trips_after_three_consecutive_denials`, `auto_reviewer_reset_clears_state` | AutoReviewer circuit breaker |
| L3-BEH-CORE-005 | `crates/core/src/context_pipeline.rs` | `assemble_builds_context_with_base_instructions`, `evaluate_skips_below_threshold`, `normalize_produces_messages` | Context assembly, compaction, normalization |
| L3-BEH-APP-003 | `specs/tests/test_l2_l3_traceability_gaps.py` | `test_classifies_unlinked_related_only_and_primary_linked`, `test_stale_target_does_not_count_as_primary_coverage`, `test_reports_malformed_rows_with_line_numbers_and_duplicate_rows`, `test_embedded_tbd_and_revision_drift_are_reported`, `test_embedded_missing_matrix_target_is_reported_for_each_target`, `test_embedded_extra_target_not_in_matrix_is_reported`, `test_blocking_mode_returns_one_for_gaps_and_advisory_returns_zero`, `test_duplicate_artifact_ids_are_usage_errors`, `test_markdown_cells_preserves_escaped_pipes` | L2-L3 traceability validator coverage classification, stale target detection, duplicate rows, malformed row diagnostics, embedded drift, advisory exit semantics, Markdown parsing |
| L3-BEH-CORE-006 | `crates/core/src/durable_record.rs` | `content_part_all_variants_roundtrip`, `mention_all_kinds_and_statuses_roundtrip`, `workspace_change_set_roundtrip` | ContentPart, Mention, WorkspaceChangeSet |
| L3-BEH-CORE-008 | `crates/core/src/instruction_discovery.rs` | `discover_project_root_finds_git_dir`, `discover_in_directory_override_priority`, `discover_in_directory_fallback_to_claude_md` | Instruction file discovery |
| L3-BEH-CORE-011 | `crates/core/src/fork.rs` | `validate_fork_request_accepts_valid_input`, `build_inherited_segment_creates_valid_descriptor` | Fork admission, segment construction |
| L3-BEH-CORE-012 | `crates/core/src/message_edit.rs` | `eligible_message_passes_check`, `active_turn_rejects_edit`, `create_edit_records_produces_edit_and_supersede` | Edit eligibility, record creation |
| L3-BEH-PROVIDER-001 | `crates/core/src/model_binding.rs` | `supported_model_definition_is_pure_capability`, `provider_error_recoverability` | Model types, error classification |
| L3-BEH-TOOLS-001 | `crates/tools/src/contracts.rs` | `tool_result_success`, `tool_call_error_recoverability`, `tool_progress_serde_roundtrip` | ToolContext, ToolResult, ToolCallError |
| L3-BEH-TOOLS-004 | `crates/tools/src/deferred_loading.rs` | `classification_exposes_preloaded_and_loaded_deferred_only`, `reminder_uses_canonical_names_not_aliases`, `tool_search_loads_aliases_and_reports_available_tools`, `tool_search_reports_already_loaded_and_not_found`, `tool_search_errors_when_all_requested_tools_are_unknown`, `loaded_tools_are_session_scoped_and_listed_deterministically`, `prompt_metrics_report_deferred_savings` | Deferred tool classification, ToolSearch executor, alias resolution, session loaded-tool tracking, prompt reminder, token metrics |
| L3-BEH-TOOLS-004 | `crates/tools/src/registry.rs`, `crates/tools/src/lib.rs` | `registry_builds_deferred_tool_prompt`, `registry_loads_deferred_tools_for_session`, `default_registry_dispatches_tool_search_and_records_loaded_tool`, `registry_from_plan_contains_all_tools_default` | Registry-facing deferred prompt assembly, loading API, default ToolSearch registration, and callable ToolSearch dispatch |
| L3-BEH-TOOLS-004 | `crates/core/src/tool_prompt.rs` | `model_surface_exposes_preloaded_tools_and_reminds_about_deferred_tools`, `model_surface_keeps_loaded_deferred_schema_and_omits_it_from_reminder` | Core model-request surface contains only preloaded plus loaded-deferred schemas and carries deferred reminder text |
| L3-BEH-SAFETY-001 | `crates/safety/src/sandbox.rs` | `default_sandbox_policy_denies_dangerous_commands`, `noop_sandbox_allows_everything`, `noop_sandbox_passes_commands_through` | Sandbox trait, SandboxPolicy |
| L3-BEH-SKILLS-001 | `crates/skills/src/parser.rs` | `valid_skill_md_parses`, `missing_name_produces_diagnostics`, `invalid_name_produces_diagnostics` | SKILL.md parsing |
| L3-BEH-SKILLS-001 | `crates/skills/src/installer.rs` | `installs_missing_skill`, `skips_existing_user_package`, `dry_run_does_not_write_files` | DefaultSkillInstaller |
| L3-BEH-SERVER-003 | `crates/server/src/subagent.rs` | `agent_registry_register_and_lookup`, `agent_path_join_and_parent`, `mailbox_send_receive` | AgentRegistry, AgentPath, SubagentMailbox |
| L3-BEH-SERVER-004 | `crates/server/src/goal.rs` | `active_goal_continues`, `turn_budget_exhausted_stops_continuation`, `goal_status_is_terminal` | Goal lifecycle, budget tracking |
| L3-BEH-SERVER-004 | `crates/server/src/runtime/handlers/goal.rs` | `goal_create_and_get`, `goal_pause_and_resume`, `goal_complete_is_terminal`, `goal_clear_removes` | GoalStore state machine |
| L3-BEH-MCP-001 | `crates/mcp/src/manager.rs` | `register_and_query_status`, `refresh_updates_status`, `invoke_tool_without_transport_errors` | InMemoryMcpManager |

### Remaining Verification Gaps

| Gap | Description |
|---|---|
| TOOLS-004 | Partial — durable loaded-tool metadata and live query/server prompt integration tests pending |
| PROVIDER-002 | No coalescence/throttling tests |
| PROVIDER-003 | No usage observability pipeline tests |
| SERVER-001 | No EventBroadcaster/sequencing tests |
| SERVER-002 | No turn_interrupt_requested durable record test |
| CLIENT-001 | No WebSocket transport tests |
| TUI-004 | No /goal integration test |
| APP-002 | No DiagnosticEvent pipeline tests |

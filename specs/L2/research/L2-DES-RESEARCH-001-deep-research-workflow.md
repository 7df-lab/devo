---
artifact_id: L2-DES-RESEARCH-001
revision: 5
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-06-30
---

# L2-DES-RESEARCH-001 — Deep Research Workflow

## Purpose

Define the v1 `/research <question>` workflow as a server-owned multi-stage research turn.

## Design

- The TUI exposes `/research <question>` as a slash command with inline arguments.
- The client sends `turn/start` with `execution_mode: "research"` and one text input; the server creates one active turn with `TurnKind::Research`.
- The workflow runs staged prompts for clarification, research brief generation, supervisor task planning, researcher notes, evidence compression, final report writing, and oversized local webpage summarization.
- At most one clarification question is asked through the existing `request_user_input` overlay using a research-specific pending request kind.
- Researcher stages use the existing `query()` loop so local and provider-hosted `web_search` / `web_fetch` behavior matches ordinary turns.
- Durable research milestones are persisted as normal `ResearchArtifact` turn items. The v1 artifact types are `clarification`, `brief`, `plan`, `finding`, `compressed_finding`, `webpage_summary`, `failure`, and `final_report_metadata`.
- Web tool calls and results are recorded as normal `ToolCall` and `ToolResult` items through the existing turn item and rollout/event replay path. The workflow does not create a separate research evidence database.
- Provider-hosted encrypted or opaque web results are preserved exactly as received. Devo does not attempt to decrypt them.
- Local fetched webpage content that exceeds the research threshold is summarized with `summarize_webpage.md` before downstream compression and report stages.
- Runtime research context includes the effective cwd in `<research_environment>` so local report paths and workspace-relative file operations resolve from the same directory as the research tools.
- Research cancellation uses the existing `turn/interrupt` active-turn interrupt path.

## Context Boundary

`/research` is a first-class turn boundary, not a prompt-only convention. The
server persists research internals for history, debugging, audit, and replay,
but later regular turns must rebuild prompt context through the shared
turn-aware projection contract.

For `TurnKind::Research`, only these items are prompt-visible to later regular
turns:

- the original `UserMessage` that started the research turn;
- the final user-facing `AgentMessage` report;
- the `ResearchArtifact` whose type is `final_report_metadata`, used as the
  compact research context reference.

Research-internal clarification artifacts, generated briefs, supervisor plans,
researcher notes, web-search/tool payloads, reasoning, webpage summaries,
compressed findings, failure artifacts, and planning traces may remain in the
rollout history and session transcript, but they are not projected into the next
normal coding model request.

Live sessions, persisted session reload, replay, resume, and compaction snapshot
rebuilds must all use the same prompt-visible predicate. Compaction preserved
item IDs are computed after this projection so research-internal tool calls and
tool results cannot be kept alive through a compacted transcript.

If research fails, is interrupted, or completes partially, the same rule applies:
only already-persisted prompt-visible research items can contribute to later
regular turns. Internal artifacts from failed, cancelled, or incomplete research
turns remain durable history only.

## Configuration

The `[research]` config section defines v1 runtime caps:

- `max_researcher_iterations`
- `fetch_summary_threshold_chars`
- `max_summary_chars`

The workflow intentionally does not define hard worker-count caps in v1. The
supervisor prompt may prefer one worker and can launch parallel workers when the
brief has clear independent subtopics. The workflow also intentionally does not
define model-stage or total-turn timeouts in v1, because model response latency
varies by provider and model.

All stages reuse the active session model, provider, reasoning effort, web search, and web fetch configuration.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-AGENT-001 | 1 | specs/L1/L1-REQ-AGENT-001-execution-workflow.md | Defines a server-owned multi-stage execution workflow for deep research. |
| refines | L1-REQ-TOOL-003 | 1 | specs/L1/L1-REQ-TOOL-003-web-search-configuration.md | Requires research to use existing local/provider-hosted web search and fetch configuration. |
| refines | L1-REQ-TUI-006 | 1 | specs/L1/L1-REQ-TUI-006-command-discovery-control.md | Defines `/research` as a discoverable slash command. |
| related-to | L2-DES-APP-003 | 2 | specs/L2/app/L2-DES-APP-003-client-server-protocol.md | Extends `turn/start` with a research execution mode and persists research milestones as turn items. |
| related-to | L2-DES-AGENT-001 | 1 | specs/L2/agent/L2-DES-AGENT-001-execution-engine.md | Reuses active-turn lifecycle, item persistence, cancellation, and usage reporting. |
| related-to | L2-DES-CONTEXT-001 | 1 | specs/L2/context/L2-DES-CONTEXT-001-context-assembly.md | Requires research turns to project only the original question, final report, and compact reference into later regular prompts. |
| related-to | L2-DES-CONTEXT-002 | 1 | specs/L2/context/L2-DES-CONTEXT-002-context-compaction.md | Requires compaction rebuilds to reuse the research prompt-visible projection contract. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 5 | 2026-06-30 | Assistant | Update | Removes research timeout caps after trial feedback showed model-specific latency makes fixed timeouts confusing. |
| 4 | 2026-06-30 | Assistant | Update | Removes unused hard worker-count caps from the v1 design and adds stage/turn timeout caps as the enforced execution boundary. |
| 3 | 2026-06-18 | Assistant | Update | Adds cwd to the prompt-visible research runtime environment for local report output. |
| 2 | 2026-06-17 | Assistant | Update | Defines the explicit `/research` context-boundary projection contract across live, replay, resume, and compaction paths. |
| 1 | 2026-06-14 | Assistant | Initial | Initial v1 deep research workflow design. |

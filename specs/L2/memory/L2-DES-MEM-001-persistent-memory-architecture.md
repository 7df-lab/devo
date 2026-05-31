---
artifact_id: L2-DES-MEM-001
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-27
---

# L2-DES-MEM-001 — Persistent Memory Architecture

## Purpose

Define the architecture for agent-maintained persistent memory — a core-internal subsystem that extracts learnings from completed sessions, consolidates them into structured memory files, and injects relevant context into future sessions. The memory system is owned by the server runtime and is not a client-managed feature.

## Scope

This document covers:
- The two-phase memory pipeline (extraction and consolidation)
- Memory filesystem layout under the user's data directory
- Phase 1: per-session raw memory extraction from session transcripts
- Phase 2: global consolidation of raw memories into structured memory files
- Read path: how memory is surfaced as developer instructions and tool-accessible context
- Configuration surface (feature flags, models, retention windows, rate limiting)
- Job-based concurrency control (claim, lease, retry, cooldown)
- Retention and pruning
- Safety: secret redaction, sandboxed consolidation, approval bypass
- Ad-hoc memory update path (user-requested writes)

This document does **not** cover:
- Individual prompt templates (deferred to L3)
- Database schema details beyond the conceptual model (deferred to L3)
- Git-based workspace diff implementation (deferred to L3)
- Secret detection engine details (see tool safety design)

## Design Decisions

### DD-1: Memory is server-owned, not client-managed

Per L1-REQ-MEM-001, persistent memory is an internal capability of the core agent runtime. Clients do not need memory list, edit, delete, export, or notification methods. This keeps the client-server protocol simple and avoids turning memory into a user-facing database.

**Decision**: All memory extraction, storage, consolidation, and read-path injection happens within the server runtime. The client protocol does not expose memory management endpoints. Session deletion may internally prune or retain linked memory entries according to internal retention policy, but clients are not required to present per-memory decisions.

### DD-2: Two-phase offline pipeline separates speed-critical extraction from quality-sensitive consolidation

Responding to every session immediately with a full consolidation pass would be expensive and noisy. Instead, a lightweight Phase 1 processes recent sessions into raw memory extracts using a fast model, and a heavier Phase 2 periodically merges accumulated raw memories into organized, searchable memory files using a more capable model.

**Decision**: Phase 1 (extraction) runs at session boundary — cheap model, low reasoning effort, parallel-friendly per-session. Phase 2 (consolidation) runs periodically — capable model, higher reasoning, single global lock, gated by rate limits and cooldown windows.

### DD-3: Memory lives as filesystem artifacts under a version-controlled directory

A filesystem-based memory store makes memory visible, inspectable, editable (for debugging), and portable. Using a git-backed workspace provides natural change detection, rollback, and audit history.

**Decision**: Memory is stored as Markdown files under `~/.devo/memories/`. The directory is git-initialized. Phase 2 operates on a workspace diff to detect what changed and avoid redundant consolidation passes. Files are structured for both human readability and model consumption.

### DD-4: Job-based distributed locking prevents duplicate work

Multiple server sessions may start simultaneously. Without coordination, multiple Phase 1 jobs could process the same session, and multiple Phase 2 jobs could compete to consolidate. A lightweight job table with ownership tokens, leases, and retry logic provides distributed coordination without an external service.

**Decision**: A `jobs` table in the local state database tracks Phase 1 and Phase 2 job state. Jobs are claimed with an ownership token and lease expiration. Concurrent workers claim disjoint jobs. Failed jobs are retried with backoff. Completed jobs enter a cooldown before the next cycle.

### DD-5: Consolidation agent runs in sandboxed isolation

Phase 2 spawns a sub-agent with locked-down permissions: workspace-write only to the memory directory, no network access, no approval prompts, no recursive tool delegation, no MCP plugins. This prevents the consolidation model from accessing user projects, leaking data, or chaining into expensive sub-operations.

**Decision**: The Phase 2 consolidation agent is configured with `approval_policy = Never`, sandbox restricted to the memory root directory only, all extensions and multi-agent features disabled, and ephemeral mode (no memory-of-memory feedback loop).

### DD-6: Memory is feature-gated and config-driven

Not all users want memory. Even for users who do, extraction cost should respect rate-limit thresholds. The entire pipeline is default-disabled and can be independently toggled for generation vs. consumption.

**Decision**: A `memories` feature flag gates the entire pipeline (default: disabled). Two sub-toggles control extraction (`generate_memories`) and injection (`use_memories`) independently. A rate-limit guard prevents extraction from consuming user-facing API quota when remaining capacity is below a configurable threshold.

## Architecture

### System Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│                         Session Lifecycle                           │
│                                                                     │
│  User session ends  ───►  Trigger Phase 1 candidate scan            │
│  (session idle for N hours)                                         │
└─────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────┐
│                     Phase 1: Raw Memory Extraction                  │
│                                                                     │
│  1. Prune stale stage-1 outputs                                     │
│  2. Guard: check rate limits > threshold                            │
│  3. Claim eligible session candidates (age + idle time filters)     │
│  4. Load filtered session transcript                                │
│  5. Stream to extraction model (fast, low reasoning)                │
│  6. Parse structured JSON output                                    │
│  7. Redact secrets from output                                      │
│  8. Store in stage1_outputs table, enqueue for Phase 2              │
│                                                                     │
│  Concurrency: up to configurable parallel jobs per pass             │
│  Claim limit: max_rollouts_per_startup per pass                     │
└─────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────┐
│                    Phase 2: Global Consolidation                    │
│                                                                     │
│  1. Claim global Phase 2 lock (lease: 3600s, cooldown: 6h)          │
│  2. Ensure memory workspace git baseline                            │
│  3. Load top-N stage-1 outputs ranked by usage + recency            │
│  4. Sync workspace: write raw_memories.md, rollout summaries        │
│  5. Prune old extension resources (>7 days)                         │
│  6. Compute git workspace diff                                      │
│  7. If no changes → mark success and exit                           │
│  8. If changes → write phase2_workspace_diff.md                     │
│  9. Spawn consolidation sub-agent (capable model, sandboxed)        │
│ 10. Heartbeat lease every 90s while agent runs                     │
│ 11. On completion: reset git baseline, mark success                 │
│ 12. Shutdown and auto-close consolidation agent                     │
└─────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────┐
│                       Read Path: Memory Injection                   │
│                                                                     │
│  On each new session start (if use_memories enabled):               │
│  1. Read memory_summary.md from memory root                         │
│  2. Render read-path instructions template                          │
│  3. Inject as developer instructions into system prompt             │
│  4. Register memory access tools (list/read/search)                 │
│                                                                     │
│  Tools operate on memory filesystem via local backend               │
└─────────────────────────────────────────────────────────────────────┘
```

### Memory Filesystem Layout

```
~/.devo/memories/                    ← git-initialized root
├── .git/                            ← git baseline for change detection
├── memory_summary.md                ← summary index (agent reads first)
├── MEMORY.md                        ← structured searchable registry
├── raw_memories.md                  ← merged Phase 1 extracts (stable thread-id order)
├── phase2_workspace_diff.md         ← temporary git diff (created before consolidation)
├── rollout_summaries/               ← per-session summary files
│   ├── 2026-05-20T14-30-00-aBcD-feature_x.md
│   └── 2026-05-21T09-15-00-eFgH-bug_fix.md
├── skills/                          ← reusable procedure packages
│   └── <skill-name>/
│       ├── SKILL.md                 ← entrypoint instructions
│       ├── scripts/                 ← optional helper scripts
│       ├── examples/                ← optional example outputs
│       └── templates/               ← optional templates
└── extensions/
    └── ad_hoc/
        └── notes/                   ← user-requested ad-hoc updates
            └── <timestamp>-<slug>.md
```

| File | Purpose | Producer | Consumer |
|------|---------|----------|----------|
| `memory_summary.md` | Concise index: user profile, preferences, general tips, what's-in-memory | Phase 2 consolidation agent | Read path: injected into system prompt |
| `MEMORY.md` | Searchable registry of task-grouped memory blocks with pointers to evidence | Phase 2 consolidation agent | Agent via memory read/search tools |
| `raw_memories.md` | Merged Phase 1 extracts, stable ascending session order | Phase 1 → Phase 2 sync | Phase 2 consolidation agent |
| `rollout_summaries/<stem>.md` | Per-session metadata + summary | Phase 2 workspace sync | Agent via memory tools, Phase 2 agent |
| `skills/<name>/SKILL.md` | Reusable procedure package | Phase 2 consolidation agent | Agent via memory tools |
| `extensions/ad_hoc/notes/<ts>-<slug>.md` | User-requested memory updates | Agent (ad-hoc write) | Phase 2 consolidation agent |

### Phase 1: Raw Memory Extraction

#### Trigger

Phase 1 runs at the start of each new root session (not sub-agents, not ephemeral sessions), in a background task, if all of the following hold:
1. `memories` feature flag is enabled
2. Session is not ephemeral
3. Session is a root (user-facing) session, not a sub-agent
4. Rate limits are above the configured threshold

#### Pipeline Steps

**a) Prune**: Remove stale `stage1_outputs` rows that have been unused beyond `max_unused_days`.

**b) Guard**: Check that the current rate-limit remaining percentage meets or exceeds `min_rate_limit_remaining_percent`. If below, skip Phase 1 to preserve user-facing quota.

**c) Claim**: Query the local state database for candidate sessions matching:
- Session source is an interactive source (CLI, not API or server-internal)
- Session is within `max_rollout_age_days` days
- Session has been idle for at least `min_rollout_idle_hours` hours
- Session `memory_mode` is enabled
- Session has not already been claimed for Phase 1 (no existing job row or retry exhausted)

Up to `max_rollouts_per_startup` candidates are claimed atomically with a lease.

**d) Extract**: For each claimed session:
1. Load the session's rollout transcript
2. Filter the transcript:
   - Exclude `developer` role messages (system instructions)
   - Exclude repetitive context fragments (large environment context blocks, skill definitions, agent instructions) to focus on user intent and model behavior
   - Exclude subagent notification markers
3. Serialize filtered transcript as structured JSON
4. Build a prompt containing the filtered transcript plus Phase 1 extraction instructions
5. Stream to the extraction model (default: fast model, low reasoning effort)
6. Parse structured JSON response into: `raw_memory` (detailed markdown), `rollout_summary` (compact single-line), `rollout_slug` (optional filename-safe tag)
7. Redact secrets from all output fields
8. Store in `stage1_outputs` table

Extraction jobs run in parallel with a configurable concurrency limit.

**e) No-output gate**: A Phase 1 response with empty `raw_memory` or empty `rollout_summary` is recorded as a success-without-output — the session contributed nothing memborable. No Phase 2 input is created.

#### Extraction Model Output Schema

```
{
  "raw_memory": string,        // Detailed markdown: what happened, decisions, outcomes
  "rollout_summary": string,   // Compact one-line summary for indexing/routing
  "rollout_slug": string|null  // Optional filename-safe tag (lowercase, alphanumeric, underscores)
}
```

### Phase 2: Global Consolidation

#### Trigger

Phase 2 runs after Phase 1 completion within the same startup task.

#### Pipeline Steps

**a) Claim global lock**: Atomically claim the global Phase 2 job. If another worker holds it (still within lease), skipped. If less than the cooldown window since last success, skipped. If retries exhausted, skipped.

**b) Prepare workspace**: Ensure `~/.devo/memories/` exists, has a git baseline. If no `.git` directory, `git init` and initial commit.

**c) Load inputs**: Query `stage1_outputs` ordered by usage count descending, recency descending, limited to `max_raw_memories_for_consolidation` rows. Exclude rows unused beyond `max_unused_days`.

**d) Sync workspace**:
- Write `raw_memories.md`: merge all loaded raw memories in stable ascending session order. Each entry includes session ID, timestamp, cwd, rollout path, and the raw memory text.
- Write `rollout_summaries/<stem>.md`: one file per session with metadata and summary.
- Prune rollout summary files no longer in the loaded set.
- Prune extension resources older than 7 days.

**e) Compute diff**: Run `git diff` to detect what changed since the last baseline.

**f) No-change gate**: If the workspace diff is empty, mark Phase 2 as successful (retaining the completion watermark) and exit — nothing to consolidate.

**g) Write diff**: Persist the diff as `phase2_workspace_diff.md` for the consolidation agent to inspect.

**h) Spawn consolidation agent**: Create a sandboxed sub-agent with:
- Config locked to workspace-write only (memory root directory)
- Network disabled
- Approval policy set to `Never`
- All extensions disabled (no MCP, no plugins, no multi-agent nesting, no further memory extraction)
- Ephemeral session mode (prevents memory recursion)
- Model: consolidation model (default: capable model with medium reasoning)
- Prompt: consolidation instructions describing the memory format, how to merge `raw_memories.md` into `MEMORY.md`, how to update `memory_summary.md`, how to create/update skills, and how to handle INIT vs. INCREMENTAL UPDATE modes

**i) Monitor with heartbeat**: While the agent runs, heartbeat the global lease every 90 seconds. If the heartbeat fails (lease lost), abort the agent.

**j) Completion**: On successful completion:
- Reset the git workspace baseline (commit the agent's changes)
- Mark the global Phase 2 job as succeeded with the new completion watermark
- Record which `stage1_outputs` were selected for this consolidation pass
- Auto-close the consolidation agent session

On failure or non-terminal status: mark the job as failed with retry delay.

### Read Path

#### Developer Instructions

When `use_memories` is enabled and a new session starts, the system injects memory usage instructions into the model's system prompt. These instructions include:

**a) Decision boundary**: When to use memory vs. skip it. Hard-skip examples: current time, simple translation, one-line shell commands. Use when the task mentions workspace content, prior context, ambiguous decisions, or non-trivial project-related work.

**b) Memory layout**: Description of the memory directory structure — the agent learns that `memory_summary.md` is already provided (embedded below), `MEMORY.md` is the primary searchable registry, and `rollout_summaries/` and `skills/` are secondary references.

**c) Quick pass protocol**: A lightweight multi-step procedure:
1. Skim the `memory_summary.md` content (already embedded in the instructions block) for task-relevant keywords
2. Search `MEMORY.md` using those keywords
3. Only if `MEMORY.md` directly points to rollout summaries or skills, open 1-2 most relevant files
4. If there are no relevant hits, stop and continue normally
5. Budget: ≤ 4-6 search steps before main work

**d) Verification guidance**: Rules for when to trust vs. verify memory-derived facts, based on drift risk and verification cost. Unverified facts must be marked as memory-derived and possibly stale.

**e) Citation format**: When memory is used, the final response must append a structured citation block identifying which memory files were used, line ranges, and what evidence was drawn from each.

**f) Ad-hoc update path**: The agent can write memory updates only when explicitly asked by the user. Updates go to `extensions/ad_hoc/notes/<timestamp>-<slug>.md` — small files describing additions, deletions, or changes. The agent must never directly edit `MEMORY.md` or `memory_summary.md`. These ad-hoc notes are picked up by the next Phase 2 consolidation pass.

#### Embedded Summary

The full content of `memory_summary.md` is embedded between markers (`MEMORY_SUMMARY BEGINS` / `MEMORY_SUMMARY ENDS`) within the developer instructions block. This gives the agent the memory overview immediately without requiring a tool call.

#### Memory Access Tools

Three read-only tools are registered when memory is enabled:

| Tool | Purpose |
|------|---------|
| `memory_list` | List memory entries (files and directories) under the memory root |
| `memory_read` | Read a memory file, with optional line offset and limit |
| `memory_search` | Search memory files for matching text with match modes (any query, all on same line, all within N lines) |

These tools use a local filesystem backend scoped to the memory root directory. They are read-only — the agent cannot modify memory files through these tools.

#### Usage Telemetry

When the agent invokes memory tools or reads memory files, usage metrics are emitted. This feeds back into the `usage_count` and `last_usage` tracking on `stage1_outputs` rows, which influences Phase 2 selection priority.

### Job-Based Concurrency Control

The pipeline uses a `jobs` table in the local state database to coordinate work across potentially concurrent server sessions.

#### Job Table Model

| Column | Role |
|--------|------|
| `kind` | Job type: `memory_stage1` or `memory_consolidate_global` |
| `job_key` | Unique key: session ID for Phase 1, `"global"` for Phase 2 |
| `status` | `pending`, `running`, `done`, `error` |
| `ownership_token` | Random token proving lease ownership |
| `started_at` | When the job was claimed |
| `lease_until` | Deadline before the lease expires |
| `retry_at` | Earliest time before a failed job can be retried |
| `retry_remaining` | Remaining retry count (decremented on failure) |
| `last_error` | Error message from the most recent failure |
| `input_watermark` | High-water mark of processed inputs |
| `last_success_watermark` | Watermark at the last successful completion |

#### Claim Semantics

**Phase 1 — claim candidate sessions**: An atomic SQL operation selects sessions matching age/idle-time/source/mode criteria, excludes those with existing job rows, inserts new job rows with `status=running`, a random ownership token, and a lease duration. The claim returns the list of claimed session metadata.

**Phase 2 — claim global**: A single-row claim on `job_key='global'`. If the current row is `done` and the cooldown has elapsed, update to `running` with a new token and lease. If `running` and lease has expired, the previous owner lost the lock — new worker can reclaim it.

#### Lease Renewal

Phase 2 heartbeat renews the lease period (default: extends lease by 3600s every 90s). If the heartbeat query returns false (row owned by a different token), the lock was lost and the consolidation agent should be aborted.

#### Retry Policy

Failed Phase 1 jobs are retried after a delay with decremented retry count. Failed Phase 2 jobs are retried after a cooldown delay. After all retries are exhausted, the job is left in `error` status and ignored until a future startup explicitly resets it.

### Retention and Pruning

| Dimension | Default | Config Key |
|-----------|---------|------------|
| Max age of source sessions | 10 days | `max_rollout_age_days` |
| Max idle time before extraction | 6 hours | `min_rollout_idle_hours` |
| Max unused days for inputs | 30 days | `max_unused_days` |
| Max raw memories per consolidation | 256 | `max_raw_memories_for_consolidation` |
| Max rollouts processed per startup | 2 | `max_rollouts_per_startup` |
| Extension resource retention | 7 days | (fixed) |

Pruning runs at the start of each Phase 1 cycle: stale `stage1_outputs` rows beyond `max_unused_days` are deleted. Phase 2 workspace sync also prunes rollout summary files that are no longer in the loaded input set.

### Safety, Security, and Privacy

#### Secret Redaction

Phase 1 extraction output passes through the secret detection and redaction engine before storage. Detected secrets are replaced with a redaction marker. This prevents credentials from being persisted into memory files and subsequently injected into future model context.

#### Consolidation Agent Isolation

The Phase 2 sub-agent is fully sandboxed:
- **Filesystem**: only the memory root directory is writable; no access to user workspaces, home directory, or system paths
- **Network**: disabled entirely
- **Approval**: `Never` (no user prompts)
- **Extensions**: all disabled (no MCP, no plugins, no skills, no multi-agent, no memory recursion)
- **Ephemeral**: session is not persisted and does not itself contribute to memory

#### Rate-Limit Guard

Phase 1 will not extract from any session if the current API rate-limit remaining percentage is below the configured threshold (default: 25%). This prevents memory extraction from competing with user-facing API calls.

#### Privacy

- Memory is stored locally under the user's home directory. It is never transmitted to external services beyond the model provider (during Phase 1 extraction and Phase 2 consolidation — and only when the feature is enabled and within rate-limit guard).
- Memory files are not exposed through the client-server protocol.
- Session deletion does not require the user to manage individual memory entries. Internally, the system may prune, retain, or unlink memory linked to a deleted session according to retention policy.

### Configuration

```toml
[features]
memories = true          # Enable the entire memory subsystem

[memories]
generate_memories = true                 # Enable Phase 1 extraction
use_memories = true                      # Enable Phase 2 consolidation + read path
max_raw_memories_for_consolidation = 256 # Max inputs per consolidation pass (1-4096)
max_unused_days = 30                     # Days before unused inputs are pruned (0-365)
max_rollout_age_days = 10                # Max age of source sessions (0-90)
max_rollouts_per_startup = 2             # Max Phase 1 claims per startup (1-128)
min_rollout_idle_hours = 6               # Min idle time before extraction (1-48)
min_rate_limit_remaining_percent = 25    # Rate-limit threshold for Phase 1 (0-100)
extract_model = "model-slug"             # Model for Phase 1 extraction (defaults to fast model)
consolidation_model = "model-slug"       # Model for Phase 2 consolidation (defaults to capable model)
```

### TUI Integration

The TUI may expose a `/memories` settings panel allowing the user to toggle `use_memories` and `generate_memories`, and to trigger a full memory reset (clear all files and database rows). This is an optional user-facing control over configuration, not a memory management protocol.

Memory state itself is not displayed in the TUI beyond the current feature toggle status.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---:|---|---|---|
| refines | L1-REQ-MEM-001 | 1 | specs/L1/L1-REQ-MEM-001-persistent-memory.md | Defines the internal memory extraction, consolidation, and read-path architecture. |
| related-to | L1-REQ-APP-012 | 1 | specs/L1/L1-REQ-APP-012-privacy-data-ownership.md | Memory is stored locally and treated as user data when model-visible. |
| related-to | L1-REQ-TOOL-001 | 1 | specs/L1/L1-REQ-TOOL-001-safety.md | Phase 1 applies secret redaction before persisting raw memories. |
| related-to | L2-DES-SAFETY-001 | 1 | specs/L2/safety/L2-DES-SAFETY-001-permission-system.md | Consolidation agent uses sandboxed workspace-write policy. |
| related-to | L2-DES-SAFETY-002 | 1 | specs/L2/safety/L2-DES-SAFETY-002-approval-mechanism.md | Consolidation agent uses Never approval to run autonomously. |
| related-to | L2-DES-AGENT-003 | 1 | specs/L2/agent/L2-DES-AGENT-003-subagent-architecture.md | Phase 2 spawns a consolidation subagent with locked-down config. |
| related-to | L2-DES-CONV-001 | 1 | specs/L2/conv/L2-DES-CONV-001-session-jsonl-data-model.md | Rollout transcripts are the source data for Phase 1 extraction. |
| related-to | L2-DES-APP-002 | 2 | specs/L2/app/L2-DES-APP-002-configuration-precedence.md | Memory configuration participates in config layering. |
| related-to | L2-DES-LLM-003 | 1 | specs/L2/llm/L2-DES-LLM-003-model-usage-observability.md | Memory pipeline usage contributes to model usage observability metrics. |
| specified-by | L3-BEH-CORE-007 | 2 | specs/L3/core/L3-BEH-CORE-007-persistent-memory.md | L3 defines extraction, consolidation, job concurrency, read-path context injection, ad-hoc updates, safety, and retention. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-27 | Assistant | Initial | Initial persistent memory architecture design. |

---
artifact_id: L2-DES-CONTEXT-001
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-22
---

# L2-DES-CONTEXT-001 — Context Assembly

## Purpose

Refine the context assembly step of the agent execution engine. Define how the immutable context prefix, metadata-derived instructions, interaction-mode prompt sets, and consolidated change-signal messages compose into model-visible context while preserving token efficiency and provider cache friendliness.

## Background / Context

`L2-DES-AGENT-001` defines context assembly as a phase of the execution engine but does not specify how metadata-derived content (persona, interaction mode, interrupt state) is ordered, deduplicated, or inserted relative to the user message. `L2-DES-CONV-001` defines session metadata fields including `persona`, `interaction_mode`, `instruction_set`, and `agent_mode`, but does not define the context assembly rules that translate those fields into model-visible content.

Four L1 requirements converge on this problem:

- Token efficiency (`L1-REQ-LLM-001`) requires a stable context prefix and append-only representation of runtime configuration changes.
- Persona (`L1-REQ-LLM-004`) requires persona instructions to influence model behavior without mutating the prefix.
- Plan Mode (`L1-REQ-AGENT-005`) requires mode-specific behavior and instruction sets that change during a session.
- Code Review (`L1-REQ-REVIEW-001`) requires a review-oriented instruction set that changes during a session.

The user has directed that Plan Mode and Review Mode share a single `interaction_mode` field with distinct prompt sets per mode, and that all pre-user-message signals (mode changes, persona changes, interruption) be consolidated into one message.

## Source Requirements

- `L1-REQ-LLM-001` requires stable context prefixes and append-only runtime configuration changes.
- `L1-REQ-LLM-004` requires persona instructions to influence model behavior.
- `L1-REQ-AGENT-005` requires Plan Mode as a session-local interaction mode with mode-specific behavior.
- `L1-REQ-GOAL-001` requires autonomous goal continuation without polluting the user-visible transcript.
- `L1-REQ-REVIEW-001` requires code review as a first-class workflow with review-specific behavior.
- `L1-REQ-CONTEXT-001` requires useful model context management across long-running sessions.
- `L1-REQ-AGENT-002` requires that interrupted work be visible as prior state when the user resumes.
- `L2-DES-CONV-001` defines the `persona`, `interaction_mode`, `agent_mode`, and `instruction_set` session metadata fields.
- `L2-DES-AGENT-001` defines the context assembly phase within the execution engine.
- `L2-DES-AGENT-002` defines interrupt and resume control, including the interrupt state used to assemble the resume signal.
- `L2-DES-GOAL-001` defines hidden goal context for Ralph Loop continuation.

## Design Requirement

The program should assemble model-visible context from four layers:

1. **Immutable prefix**: Stable content that must not be rewritten in-place, including base instructions, tool definitions, and prior transcript turns.
2. **Metadata-derived content**: Persona instructions and interaction-mode instructions assembled from session metadata for every turn.
3. **Hidden goal context**: Active Ralph Loop goal context inserted only when goal continuation is eligible for the current turn.
4. **Consolidated change-signal message**: A single message inserted before the user input when persona, interaction mode, goal state, or interrupt state changed since the prior turn.

These layers compose into the final model context. The immutable prefix preserves provider cache reuse. Metadata-derived content allows dynamic configuration without prefix mutation. Hidden goal context keeps Ralph Loop continuation out of the visible transcript. The consolidated signal avoids redundant messages while keeping the model informed of changed circumstances.

## Interaction Mode

The session carries an `interaction_mode` field that represents the current session-local interaction mode. Values are:

| Value | Behavior |
|---|---|
| `normal` | Full agent capabilities. Question tool blocked. Mutating tools available subject to permission and safety policy. |
| `plan` | File mutation blocked. Question tool available. Agent produces strategic analysis and plans. Non-mutating inspection tools available. |
| `review` | File mutation blocked. Agent inspects code and produces prioritized findings. Code-location and reasoning required per finding. |

`interaction_mode` is distinct from the session-level `agent_mode` field (Coding Mode, Security Mode) defined by `L2-DES-CONV-001`. The session-level agent mode is locked at session creation. `interaction_mode` may change during a session.

Each `interaction_mode` value maps to a distinct prompt set that defines mode-specific instructions, constraints, and output expectations. The prompt set is metadata-owned and is not a user transcript item.

The mode prompt set for `plan` should include:
- Prohibition on file creation, editing, deletion, renaming, or other mutation.
- Permission to read files, search the codebase, and inspect project context.
- Requirement to produce a strategic, actionable plan from user input and analysis.
- Permission to ask clarification questions through the question tool.
- Instruction that the output is a plan, not an implementation.

The mode prompt set for `review` should include:
- Prohibition on file creation, editing, deletion, renaming, or other mutation.
- Requirement to inspect the relevant code, diff, branch, commit, or pull request context.
- Requirement to lead with findings ordered by severity.
- Requirement to include file or code location and risk reasoning per finding.
- Instruction to state clearly when no issues are found and identify remaining verification gaps.
- Instruction not to modify code unless the user explicitly requests fixes.

The mode prompt set for `normal` should include:
- Standard agent capabilities without mode-specific additions.
- No question tool availability (blocked at the tool gate, not just instruction level).

## Immutable Context Prefix

The immutable context prefix is the portion of model-visible context that must not be rewritten in-place between turns. It includes:

- Base instructions.
- Session-level agent mode instructions (e.g., Coding Mode or Security Mode base prompts).
- Tool definitions and schemas.
- Prior transcript turns and their items.
- Prior durable metadata-derived content that has already been sent to the model (see Change-Signal Rules below).

The immutable prefix may grow as new instructions, tool definitions, or transcript turns are appended. It must not have existing content rewritten, reordered, or mutated when downstream configuration changes.

When context compaction produces a summary, the summary replaces the compacted transcript range in future context snapshots. This is not a prefix mutation — it is a new context snapshot that begins after the stable prefix and references summaries instead of individual turns. The previously sent prefix content is not rewritten.

## Metadata-Derived Content

Each turn, the context assembler derives model-visible content from session metadata. This content is not a transcript turn and is not persisted as a user or assistant message in durable storage.

The metadata-derived content includes:

- **Persona instructions**: The instruction text associated with the current persona selection, such as concise style, detailed style, or other configured communication-style instructions.
- **Interaction-mode instructions**: The mode-specific prompt set for the current `interaction_mode` value.
- **Goal context**: The active Ralph Loop objective, status, budget, and progress summary when the current turn is eligible for goal continuation.

These instructions are assembled from the `instruction_set` and related metadata fields defined by `L2-DES-CONV-001`. They are included in the model-visible context for every turn.

## Hidden Goal Context

Hidden goal context is model-visible context derived from active goal state. It is not a user-visible transcript item.

Rules:

- It should be inserted only when the session has an active goal and the current turn is eligible for goal-guided execution.
- It should be suppressed in Plan Mode unless a later L3 design explicitly defines a read-only planning interaction with goals.
- It should include the untrusted user objective, budget state, progress, and completion-audit instructions.
- User-provided objective text must be escaped before being embedded in structured tags.
- The exact hidden goal context or a stable reference to it should be captured in the context snapshot or in a goal context snapshot record.

## Consolidated Change-Signal Message

When the state of persona, interaction mode, goal context, or interrupt condition changes between turns, the context assembler generates one consolidated change-signal message inserted before the user input. This message bundles all active changes into a single model-visible signal.

The change-signal message is generated when any of the following differ from the prior turn's context state:

- `persona` has changed.
- `interaction_mode` has changed.
- Active goal state relevant to model-visible context has changed.
- The prior turn was interrupted by the user.

If none of these changed, no change-signal message is generated. The metadata-derived persona, mode, and hidden goal context already reflect the current state where they are eligible.

The change-signal message should be concise and factual:

- State which persona is now active.
- State which interaction mode is now active.
- State that the active goal changed, paused, resumed, completed, blocked, or stopped by budget, if applicable.
- State that the prior turn was interrupted, if applicable.

Example shape:

```text
[SYSTEM — change signal]

The persona is now: concise.
The interaction mode is now: plan.
The active goal was paused by the user.
The previous turn was interrupted by the user.

All subsequent responses should use the current persona, interaction mode, and goal state.
```

All active changes are stated in one message. The program must not emit separate messages for the persona change, the mode change, and the interruption.

The change-signal message is metadata-derived content. It is not a transcript turn and is not persisted as a user or assistant message in durable storage. It is regenerated during context assembly when replaying or continuing a session.

### Ordering

When a change-signal message is generated, the ordering within the model-visible context for the current turn is:

```text
[Immutable prefix]
[Metadata-derived: persona instructions (current)]
[Metadata-derived: interaction-mode instructions (current)]
[Hidden goal context, if eligible]
[Consolidated change-signal message, if applicable]
[User input — the current turn's accepted user message]
```

The change-signal message appears after the metadata-derived instructions and immediately before the user input. This ensures the model sees the current instructions, then the signal explaining what changed, then the user's request.

### Interaction with Context Compaction

When context compaction summarizes earlier turns, the change-signal message for those earlier turns may be summarized or omitted depending on whether the signal's information remains relevant. The current turn's context assembly is not affected by compaction of earlier change-signal messages — the assembler always generates the current turn's signal from current metadata.

## Interrupt Signal

When the prior turn was interrupted by the user, the change-signal message includes an interrupt statement. The statement should be factual and not imply the model's prior output was wrong.

The interrupt signal does not carry the interrupted turn's partial content. The immutable prefix already includes the interrupted turn's durable records (partial assistant output, tool results, workspace change state). The signal only informs the model that the prior turn was interrupted.

When a turn is interrupted and the user immediately submits a new message, the change-signal message bundles the interrupt notification with any concurrent persona or mode changes. If the user changes persona or mode before resubmitting, those changes appear in the same consolidated message.

## Context Assembly Flow

For each new turn, the context assembler:

1. Load the immutable prefix from the current context snapshot.
2. Detect whether persona, interaction mode, active goal context, or interrupt condition changed since the prior turn's assembled context.
3. Assemble the metadata-derived persona instructions from current session metadata.
4. Assemble the metadata-derived interaction-mode instructions from current session metadata.
5. Assemble hidden goal context when the current turn is eligible for goal-guided execution.
6. If a change is detected, generate one consolidated change-signal message.
7. Insert the accepted user input after the metadata-derived content, optional hidden goal context, and optional change signal.
8. Serialize the assembled context into provider-specific request messages (system, developer, user, assistant, tool messages).
9. Record a context snapshot reference for future turns.

Step 8 is a provider-specific serialization concern and does not convert metadata-derived content, hidden goal context, or change-signal messages into transcript turns. The durable session record does not store the assembled context as transcript items.

## Prefix Stability and Token Efficiency

This design satisfies token-efficiency requirements because:

- The immutable prefix is never rewritten in-place when configuration changes.
- Persona and mode changes are represented by appended metadata-derived content and an optional change-signal message, not by editing previously sent prefix content.
- Goal context is appended as hidden metadata-derived content when needed; it does not rewrite earlier transcript or instruction records.
- Interruption state is represented by the change-signal message, not by mutating the prior turn's durable records.
- The change-signal message is consolidated — one message regardless of the number of changes — avoiding redundant token consumption.
- When no metadata changes occur between turns, no change-signal message is generated at all.

Provider prefix caching should benefit from stable base instructions, tool definitions, and prior transcript content that remain byte-identical across turns.

## Persona, Mode, and Review as Metadata

This design treats persona, interaction mode, and review behavior as metadata-derived instructions, not as transcript items. This satisfies:

- Persona changes influence model behavior without creating user-visible transcript turns.
- Plan Mode and Review Mode share the `interaction_mode` field with distinct prompt sets.
- Review findings and plan output are normal assistant response items within the transcript. The mode instructions that produce those findings are metadata-derived, not user-authored transcript content.
- Users can switch between normal, plan, and review modes within a session without creating synthetic transcript items just to represent the mode change.
- Active goal context can guide autonomous continuation without creating synthetic user transcript items.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---:|---|---|---|
| refines | L1-REQ-LLM-001 | 1 | specs/L1/L1-REQ-LLM-001-token-efficiency.md | Defines immutable prefix, append-only metadata changes, and consolidated change-signal for cache-friendly context. |
| refines | L1-REQ-LLM-004 | 1 | specs/L1/L1-REQ-LLM-004-persona.md | Defines persona as metadata-derived instruction with append-only change signaling. |
| refines | L1-REQ-AGENT-005 | 1 | specs/L1/L1-REQ-AGENT-005-plan-mode.md | Defines plan as an interaction_mode value with a mode-specific prompt set and consolidate change signal. |
| related-to | L1-REQ-GOAL-001 | 1 | specs/L1/L1-REQ-GOAL-001-ralph-loop.md | Defines hidden goal context as model-visible metadata-derived content rather than a user-visible transcript turn. |
| refines | L1-REQ-REVIEW-001 | 1 | specs/L1/L1-REQ-REVIEW-001-code-review.md | Defines review as an interaction_mode value with a mode-specific prompt set sharing the mode field. |
| related-to | L1-REQ-CONTEXT-001 | 1 | specs/L1/L1-REQ-CONTEXT-001-management.md | Context assembly produces the model-visible context managed by the context management system. |
| related-to | L1-REQ-AGENT-002 | 1 | specs/L1/L1-REQ-AGENT-002-interrupt-resume.md | Interrupt state informs the consolidated change-signal message before the next user input. |
| related-to | L2-DES-CONV-001 | 1 | specs/L2/conv/L2-DES-CONV-001-session-jsonl-data-model.md | Session metadata fields (persona, interaction_mode, instruction_set) provide the source data for context assembly. |
| related-to | L2-DES-AGENT-001 | 1 | specs/L2/agent/L2-DES-AGENT-001-execution-engine.md | Refines the context assembly phase of the execution engine. |
| related-to | L2-DES-AGENT-002 | 1 | specs/L2/agent/L2-DES-AGENT-002-interrupt-resume-control.md | Interrupt state feeds the consolidated change-signal when resuming after interruption. |
| related-to | L2-DES-GOAL-001 | 1 | specs/L2/goal/L2-DES-GOAL-001-ralph-loop-goals.md | Defines goal context content, eligibility, and persistence expectations. |
| specified-by | L3-BEH-CORE-005 | 2 | specs/L3/core/L3-BEH-CORE-005-context-pipeline.md | L3 defines context assembly ordering, immutable prefix handling, metadata instructions, hidden goal context, and change signals. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-22 | Assistant | Initial | Initial context assembly design refining token efficiency, persona, plan mode, and code review into immutable prefix, metadata-derived content, and consolidated change-signal message. |
| 1 | 2026-05-23 | Human | Refinement | Added hidden Ralph Loop goal context as metadata-derived model-visible content. |

---
artifact_id: L3-BEH-SERVER-005
revision: 3
status: Draft
active_baseline: no
---

# L3-BEH-SERVER-005 — Skills Discovery and Activation

## Purpose

Define the runtime behavior for discovering skill packages from configured roots, maintaining the effective skill catalog, handling user-explicit and model-selected activation, loading skill instructions into context, and enforcing skill trust/safety boundaries. Skill package definitions, `SKILL.md` parsing, bundled default skills, and default-skill installation are defined by `L3-BEH-SKILLS-001`.

## Source Design

L2-DES-SKILLS-001 (Agent Skills Architecture), L3-BEH-SKILLS-001 (Skill Package Definitions and Default Installation)

## Behavior Specification

### B1. Skill Discovery from Roots

- **Trigger**: Server starts, configuration changes, or explicit `skills.refresh` request.
- **Preconditions**: Skill feature is enabled. Discovery roots are configured.
- **Algorithm / Flow**:
  1. Ensure bundled default skills have been installed or checked by calling the `skills` crate default-skill installer. Installation failures become catalog diagnostics and must not abort server startup.
  2. Collect discovery roots from config (defaults and user-configured):
     - `~/.devo/skills/` (user native)
     - `~/.agents/skills/` (user interoperability)
     - `<workspace>/.devo/skills/` (workspace native)
     - `<workspace>/.agents/skills/` (workspace interoperability)
     - Plugin-provided roots from installed plugins.
  3. For each root, scan immediate subdirectories (depth 1).
  4. For each subdirectory: check for `SKILL.md`. If present and readable, pass it to the `skills` crate parser.
  5. Convert parsed `SkillPackage` values into runtime `SkillCatalogEntry`: `skill_id` (generated from source+name), `name`, `description`, `source` (builtin/user/workspace/plugin), `package_root`, `entrypoint_path`, `enabled`, `trust_state` (trusted for builtin/user, untrusted for workspace by default), `version`, `tags`, `diagnostics`.
  6. Preserve package parser diagnostics from `L3-BEH-SKILLS-001`.
  7. Handle duplicates: if same `name` appears in multiple sources, higher-precedence source wins (user > workspace > builtin). Lower-precedence entry is marked as `shadowed`.
  8. Bound total discovered skills and metadata bytes for context efficiency.
- **Postconditions**: The skill catalog is populated with all discoverable skills. Diagnostics are attached for invalid entries.

### B2. Skill Activation — User-Explicit

- **Trigger**: User submits a structured `Mention(kind = Skill)` selected from the `@` fuzzy-search popup, or a future explicit skill-activation protocol method is approved.
- **Preconditions**: The skill exists in the catalog. It is enabled.
- **Algorithm / Flow**:
  1. Resolve the skill by the mention target skill id or canonical skill name. If not found → error "Skill '<name>' not found. Available: <list>".
  2. If disabled: error "Skill '<name>' is disabled."
  3. If workspace-provided and `trust_state: untrusted`: warn user with source and trust info. Require explicit confirmation before activation (one-time per session).
  4. Load the full `SKILL.md` content. Apply size bounding (max 65536 bytes).
  5. Extract instructions from the skill body (content after frontmatter).
  6. Inject into context as task-scoped metadata-derived instructions through the context pipeline (`L3-BEH-CORE-005`). Mark as `requested_by: user`.
  7. Register activation: record in session state (`active_skills` list) and append durable `skill_activated` record.
  8. Broadcast `skill_activated` client event.
- **Postconditions**: Skill instructions are active for the current task. The user is informed of activation.

### B3. Skill Activation — Model-Selected

- **Trigger**: Model selects a skill from the concise catalog via a controlled activation tool or equivalent path.
- **Preconditions**: The model's context includes a brief skill catalog (name + description only). The skill is enabled.
- **Algorithm / Flow**:
  1. Model invokes the skill activation path with `skill_id` and `activation_reason`.
  2. Validate: skill exists, is enabled, and `trust_state` allows automatic activation (builtin/user skills: allowed; workspace skills: require prior user trust grant).
  3. Load `SKILL.md` content and inject into context as task-scoped instructions. Mark as `requested_by: model`.
  4. Record activation and broadcast event.
  5. The assistant is instructed to tell the user which skill was activated and why (visibility requirement).
- **Postconditions**: Model-selected skills follow the same safety and context rules as user-explicit skills.

### B4. Skill Instruction Precedence

- **Trigger**: Skill instructions are injected into context alongside other instruction sources.
- **Preconditions**: Multiple instruction sources may overlap (base instructions, persona, mode, project instructions, skills).
- **Algorithm / Flow**:
  1. Skill instructions are inserted at a lower priority than:
     - System and developer instructions.
     - Safety and permission policy.
     - The user's current request.
     - Project instruction files (`AGENTS.md`, etc.).
     - Current interaction mode instructions.
  2. Skill instructions may specialize HOW work is performed only within those boundaries.
  3. Multiple active skills: user-explicit skills in user-specified order, then model-selected skills in activation order.
  4. A skill cannot: grant tool permissions, disable approval, override user constraints, change privacy policy, or require the assistant to hide its use.
- **Postconditions**: Skills enhance agent behavior within safe boundaries.

### B5. Supporting File and Script Access

- **Trigger**: Skill instructions reference a supporting file (`references/`, `scripts/`, `assets/`) within the skill package.
- **Preconditions**: The skill is activated. The referenced file is within the skill package root.
- **Algorithm / Flow**:
  1. Supporting files are NOT loaded during discovery — only on explicit read.
  2. Relative paths in skill instructions resolve inside the skill package root.
  3. Reading supporting files uses normal file-read tools with output limits and redaction.
  4. Running scripts uses normal command execution with approval, sandbox, and workspace policy.
  5. Generated artifacts from skill scripts are ordinary workspace changes attributed to the active turn.
- **Postconditions**: Skill resources are accessed through normal, auditable tool calls.

### B6. Skill Refresh and Change Handling

- **Trigger**: Watched skill root changes, or explicit `skills.refresh`.
- **Preconditions**: File watchers are active on configured skill roots.
- **Algorithm / Flow**:
  1. On change (debounced 500ms): re-scan affected roots. Update catalog incrementally.
  2. If a skill's `SKILL.md` content changes while it's activated: do NOT silently replace already-injected instructions. A later turn may load the new version with a visible activation record.
  3. If a skill is removed from disk: mark as `unavailable` in catalog. Active sessions retain the loaded instructions.
  4. Refresh is atomic from context assembly perspective: use last successful catalog during assembly, apply refresh to later turns.
- **Postconditions**: The catalog stays current. Active sessions are not disrupted.

## Traceability

| L2 Source | Relationship |
|---|---|
| L2-DES-SKILLS-001 | specified-by |
| L3-BEH-SKILLS-001 | related-to |

## Implementation Placement Guidance

- Skill package data types, `SKILL.md` frontmatter parsing, package validation, bundled default skills, and default installation belong to the `skills` crate.
- Runtime skill catalog ownership belongs to core; `crates/core/src/skills.rs` is a conventional placement if it follows this L3 contract. Activation orchestration is server runtime behavior.
- Duplicate resolution: a `HashMap<SkillName, SkillCatalogEntry>` with source precedence ordering. Lower-priority duplicates are kept in a `shadowed` list for diagnostics.
- Skill activation records are durable JSONL records: `skill_activated { skill_id, name, source, requested_by, turn_id, activated_at }`.

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-27 | Assistant | Initial | Initial skills discovery and activation behavior. |
| 2 | 2026-05-27 | Assistant | Correction | Split package parsing and default-skill installation into `L3-BEH-SKILLS-001`; runtime catalog and activation policy remain here. |
| 3 | 2026-05-27 | Assistant | Correction | Replaced typed `@skill:` activation examples with structured Skill mentions from the direct-keyword `@` fuzzy-search flow. |

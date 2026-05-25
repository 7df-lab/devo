---
artifact_id: L2-DES-SKILLS-001
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-25
---

# L2-DES-SKILLS-001 - Agent Skills Architecture

## Purpose

Define the technical design for discovering, presenting, selecting, loading, and applying Agent Skills as reusable instruction packages.

## Background / Context

Agent Skills are reusable packages centered on a `SKILL.md` file. The external Agent Skills guidance emphasizes progressive disclosure: expose concise metadata first, load the full skill instructions only when relevant, and let the skill package refer to supporting scripts, references, and assets that can be loaded on demand.

The program should use skills to improve task behavior without letting skill content override the user's current request, safety policy, project instructions, or approval boundaries.

## Source Requirements

- `L1-REQ-APP-009` requires skill discovery, explicit skill requests, visible skill use, and clear missing-skill handling.
- `L1-REQ-APP-010` requires persistent configuration and unavailable-state behavior.
- `L1-REQ-WORKSPACE-001` requires workspace context that respects local project state.
- `L1-REQ-CONTEXT-001` requires useful model context management.
- `L1-REQ-LLM-001` requires token-efficient context construction.
- `L1-REQ-LLM-004` requires communication behavior to remain controlled by configured instructions.
- `L1-REQ-TOOL-001` requires safety, approval, and redaction for actions triggered while using skills.
- `L2-DES-APP-002` defines configuration precedence.
- `L2-DES-APP-003` defines client/server protocol visibility.
- `L2-DES-CONTEXT-001` defines metadata-derived context assembly.
- `L2-DES-TOOL-001` defines the tool system used for skill activation and related file/script work.
- `L2-DES-WORKSPACE-001` defines project instruction discovery, which remains separate from skills.

## Design Requirement

The program should provide a skill catalog, skill resolver, and skill activation workflow.

The catalog discovers skill metadata from configured roots and exposes only concise name/description/source information by default. The resolver loads a skill package when the user explicitly requests it or when the model selects it through a controlled activation path. The activated skill becomes task-scoped guidance in model context and may reference package files for on-demand reading, but it does not gain authority over safety, tools, or user intent.

## Skill Package Model

A skill package should be a directory containing a required `SKILL.md` file and optional supporting files.

Conceptual package layout:

```text
skill-name/
  SKILL.md
  references/
  scripts/
  assets/
```

`SKILL.md` is the entrypoint. It should contain frontmatter followed by instructions. Required and recommended metadata should follow the Agent Skills specification where possible.

Conceptual frontmatter fields:

- `name`: stable skill name.
- `description`: concise description used for discovery and model selection.
- `version`: optional package version.
- `enabled`: optional local enablement marker.
- `tags`: optional categorization.
- `compatibility`: optional client or model compatibility hints.
- `allowed_tools`: optional advisory list of tools the skill may need.

Only `name` and `description` should be required for a package to be discoverable as a normal skill. Missing or malformed optional fields should produce diagnostics, not hard failures, unless a later L3 validator requires stricter authoring mode.

Supporting files are package resources. They must not be eagerly loaded into model context just because the skill exists.

## Skill Sources

The catalog should support multiple source scopes:

| Source | Purpose |
|---|---|
| Built-in | Skills shipped with the program. |
| User | Skills installed for the current user across workspaces. |
| Workspace | Skills committed to or placed inside the active workspace. |
| Plugin | Skills contributed by installed plugins. |
| External package | Skills installed from a package or repository by an explicit user action. |

User and project scopes follow configuration precedence from `L2-DES-APP-002`. Workspace skills are useful but potentially untrusted, because opening a repository should not silently grant that repository authority to steer the agent.

## Discovery Roots

Discovery roots are configuration-driven. Recommended default roots:

- User native root: `~/.devo/skills/`
- User interoperability root: `~/.agents/skills/`
- Workspace native root: `<workspace>/.devo/skills/`
- Workspace interoperability root: `<workspace>/.agents/skills/`
- Plugin-provided skill directories from installed plugin metadata.

The concrete TOML shape for persisted skill enablement and discovery roots is defined by `L2-DES-APP-005` under `[skills]` and `[skills.roots.<root_id>]`.

Discovery rules:

- Scan only configured skill roots and immediate package directories unless a root explicitly declares another layout.
- Ignore unrelated large build or dependency directories.
- Require a `SKILL.md` entrypoint for normal package discovery.
- Treat non-UTF-8 or unreadable `SKILL.md` files as invalid and report diagnostics.
- Canonicalize paths before comparing roots and package identities.
- Bound the number of discovered skills and the total metadata bytes returned to context.
- Do not read supporting package files during catalog discovery.

If the same skill name appears in multiple sources, resolution must be deterministic and visible. The design should prefer explicit source priority over silent replacement. A duplicate can be represented as an error, a shadowed lower-priority record, or a user-resolved conflict, but it must not be ambiguous to the model.

## Skill Catalog

The skill catalog is the discovery output consumed by clients, context assembly, and activation.

Conceptual `SkillCatalogEntry` fields:

- `skill_id`: stable local identifier.
- `name`
- `description`
- `source`
- `package_root`
- `entrypoint_path`
- `enabled`
- `trust_state`
- `version`
- `tags`
- `compatibility`
- `diagnostics`
- `last_loaded_at`
- `last_changed_at`

The model-visible catalog should usually include only `name`, `description`, and a stable activation identifier. Full paths, diagnostics, and source details are client-visible or debug-visible but should not consume routine model context unless needed.

## Activation Paths

Skills can be activated in two ways:

1. User-explicit activation: the user names or selects a skill.
2. Model-selected activation: the model selects a skill from the concise catalog through a controlled activation tool or equivalent runtime path.

User-explicit activation has priority because it is part of the user's current intent. If the user asks for a missing skill, the program should explain that it is unavailable and continue only if the task can be performed without it.

Model-selected activation must be mediated by the runtime. The model should not be expected to search arbitrary directories and decide that a file is a skill. A dedicated activation path allows the program to enforce trust, source, availability, diagnostics, token limits, and audit records.

Conceptual activation input:

- `skill_id`
- `activation_reason`
- `requested_by`: user, model, client, or automation.
- `turn_id`
- `workspace_root`

Conceptual activation result:

- `skill_id`
- `skill_name`
- `source`
- `entrypoint_content`
- `package_root`
- `available_supporting_files`
- `diagnostics`
- `loaded_at`

The assistant should tell the user when a skill is being used and why, satisfying the visibility requirement from `L1-REQ-APP-009`.

## Context Integration

Skill context follows progressive disclosure.

Context assembly should include:

- A bounded catalog of available skills when skill use is enabled and relevant.
- Full `SKILL.md` content only for activated skills.
- Supporting file content only when explicitly read or selected by the skill instructions through normal tools.
- A concise activation record so replay can explain why the skill was present.

Activated skill instructions are task-scoped metadata-derived content. They are not user transcript items and should not silently rewrite prior context. If a skill is activated after a turn has started, the activation applies to the next model invocation or next turn according to the execution state.

If multiple skills are activated for one task, their order must be deterministic:

1. User-explicit skills in user-specified order.
2. Runtime-required skills.
3. Model-selected skills in activation order.

Conflicts are resolved by instruction precedence and, where equal, later activation does not silently override earlier active skill guidance without a visible activation record.

## Instruction Precedence

Skill instructions are lower priority than:

- System and developer instructions.
- Safety and permission policy.
- The user's current request.
- Explicit project instruction files.
- Current interaction mode instructions.
- Active configuration and permission posture.

Skill instructions may specialize how work is performed only inside those boundaries. A skill cannot grant tool permissions, disable approval, override user constraints, change privacy policy, or require the assistant to hide its use.

## Supporting Files And Scripts

Skills may contain scripts, references, templates, examples, assets, or other package files. These files are package resources.

Rules:

- Supporting files are not loaded during normal discovery.
- Relative paths mentioned by `SKILL.md` resolve inside the skill package root unless explicitly allowed otherwise.
- Reading supporting files uses normal file-read behavior with output limits and redaction.
- Running scripts uses normal command execution, approval, and workspace policy.
- A skill's `allowed_tools` or similar metadata is advisory. It may help the model choose tools, but it does not authorize tool use.
- Generated artifacts from a skill are ordinary workspace changes and must be attributed to the active turn.

## Trust And Safety

Skills are instruction packages and may contain prompt-injection attempts, unsafe commands, stale guidance, or misleading descriptions.

Safety rules:

- Workspace-provided skills require trust-aware visibility before automatic model activation.
- User-explicit skill activation may load an untrusted skill only with clear source visibility where policy requires it.
- Skills cannot override higher-priority instructions.
- Skills cannot make hidden network, filesystem, or command actions happen without normal tool calls.
- Skill descriptions are selection hints, not trusted policy.
- Skill content must be bounded and redacted before model insertion.
- Skill package paths and diagnostic details are safe client/debug projections, not routine model context.
- Skill activation and supporting-file reads should be durable enough for replay and audit.

## Client Visibility

Clients should expose skill state without forcing users to inspect filesystem paths.

Client projections should include:

- Skill name and description.
- Source scope.
- Enabled or disabled state.
- Trust state.
- Diagnostics for missing, invalid, duplicate, or incompatible skills.
- Active skills for the current turn or task.
- Last refresh time.

Representative protocol surfaces may include:

- `skills.list`
- `skills.refresh`
- `skills.activate`
- `skills.deactivate`
- `skills.inspect`

Activation/deactivation are session or task state changes. They should produce server-client events so every connected client can render the same active skill state.

## Refresh And Change Handling

Skill discovery should refresh when:

- The effective configuration changes.
- The active workspace changes.
- A watched skill root changes.
- The user explicitly requests refresh.
- A skill activation attempts to load a stale entry.

Refresh should be atomic from the perspective of context assembly. If discovery is in progress, the runtime may use the last successful catalog and apply the refresh to later turns.

If an activated skill changes on disk during a session, the runtime should not silently replace already-injected content. A later turn may load the new version with a visible refresh or activation record.

## Error Handling

Skill errors should be normalized into stable categories:

- Discovery disabled.
- Root unavailable.
- Skill not found.
- Skill disabled.
- Skill untrusted.
- Skill incompatible.
- Invalid metadata.
- Entrypoint unreadable.
- Duplicate skill name.
- Supporting file unavailable.
- Activation rejected by policy.
- Content exceeds limit.

Errors should be actionable. For example, a missing explicitly requested skill should name the missing skill and the active discovery roots where appropriate, without dumping irrelevant filesystem data into model context.

## Invariants

- A skill is discovered from metadata before full content is loaded.
- Full skill instructions enter context only after activation.
- Supporting files are loaded on demand, not during catalog discovery.
- Skill use is visible to the user.
- Missing or invalid skills do not fail the whole session unless the requested task depends on that skill.
- Skill instructions cannot override higher-priority instructions, safety, or approval rules.
- Skill activation is bounded, auditable, and replayable.
- Duplicate skill names are resolved deterministically or reported as conflicts.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-APP-009 | 1 | specs/L1/L1-REQ-APP-009-skills.md | Defines skill package discovery, activation, context integration, trust, and visibility behavior. |
| related-to | L1-REQ-APP-010 | 1 | specs/L1/L1-REQ-APP-010-configuration.md | Skill roots, enablement, and refresh behavior are configuration-driven. |
| related-to | L1-REQ-WORKSPACE-001 | 1 | specs/L1/L1-REQ-WORKSPACE-001-project-context.md | Workspace skills are part of workspace context but remain separate from project instruction files. |
| related-to | L1-REQ-CONTEXT-001 | 1 | specs/L1/L1-REQ-CONTEXT-001-management.md | Skill catalog and activated skill content participate in context management. |
| related-to | L1-REQ-LLM-001 | 1 | specs/L1/L1-REQ-LLM-001-token-efficiency.md | Progressive disclosure avoids injecting every skill body into every request. |
| related-to | L1-REQ-LLM-004 | 1 | specs/L1/L1-REQ-LLM-004-persona.md | Skill instructions must not override configured persona and communication behavior. |
| related-to | L1-REQ-TOOL-001 | 1 | specs/L1/L1-REQ-TOOL-001-safety.md | Scripts and tool use triggered by skills remain subject to safety and approval. |
| related-to | L2-DES-APP-002 | 1 | specs/L2/app/L2-DES-APP-002-configuration-precedence.md | Configuration precedence resolves skill roots and enablement. |
| related-to | L2-DES-APP-005 | 1 | specs/L2/app/L2-DES-APP-005-config-toml-schema.md | Defines concrete TOML fields for skill enablement and discovery roots. |
| related-to | L2-DES-APP-003 | 1 | specs/L2/app/L2-DES-APP-003-client-server-protocol.md | Clients inspect and receive events for skill discovery and activation state. |
| related-to | L2-DES-CONTEXT-001 | 1 | specs/L2/context/L2-DES-CONTEXT-001-context-assembly.md | Activated skills are task-scoped metadata-derived context. |
| related-to | L2-DES-TOOL-001 | 1 | specs/L2/tool/L2-DES-TOOL-001-built-in-tool-system.md | Skill activation and supporting file/script use flow through controlled tools. |
| related-to | L2-DES-WORKSPACE-001 | 1 | specs/L2/workspace/L2-DES-WORKSPACE-001-project-instruction-discovery.md | Project instruction files and workspace skills are separate context sources. |
| specified-by | TBD | TBD | specs/L3/skills/TBD.md | L3 behavior has not been authored yet. |

## References

- [Agent Skills](https://agentskills.io/)
- [Agent Skills specification](https://agentskills.io/specification)
- [Adding skills support to agents](https://agentskills.io/client-implementation/adding-skills-support)

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-25 | Assistant | Initial | Initial Agent Skills architecture based on Agent Skills reference documentation and product requirements. |
| 1 | 2026-05-25 | Human | Refinement | Linked skill configuration to the concrete `config.toml` schema. |

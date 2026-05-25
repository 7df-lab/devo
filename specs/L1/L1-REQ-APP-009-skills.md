---
artifact_id: L1-REQ-APP-009
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Human
last_updated: 2026-05-20
---

# L1-REQ-APP-009 — Skills

## Purpose

Let users extend agent behavior with reusable instruction bundles.

## Why This Matters

Skills let users reuse specialized workflows without re-explaining them in every prompt. They must remain discoverable and subordinate to the user's current intent and safety constraints.

## Background / Context

Skills capture specialized workflows, domain instructions, and reusable procedures. They must be discoverable and used intentionally.

## User / Business Requirement

The program must support skills as user-visible reusable capability packages.

## Real User Scenarios

- A user asks the program to use a known skill for a document, frontend, or repository-specific workflow.
- A workspace provides a skill, and the user wants to know whether it was discovered and applied.

## Functional Requirements

- The user must be able to discover available skills.
- The user must be able to reference or request a skill for a task.
- The program must explain when a skill is being used and why it is relevant.
- The program must handle missing, invalid, or unavailable skills clearly.

## Non-Functional Requirements

- Skill use must not override higher-priority safety or user instructions.
- Skill discovery must respect configured roots and workspace boundaries.

## Acceptance Criteria

- Given an available skill, when the user requests it, then the program applies the skill or explains why it cannot.
- Given a missing skill, when the user requests it, then the program reports that it is unavailable without failing the whole session.
- Given a skill is applied, when the program starts the task, then the user can see that the skill is being used.
- Given a skill conflicts with higher-priority instructions, when the task runs, then higher-priority instructions win.

## Out of Scope

- The program does not define skill file format, installation workflow, or runtime injection mechanics in this L1 requirement.
- This requirement does not allow skills to override user approval, safety, or privacy boundaries.

## Open Questions

- Should skills be automatically selected, explicitly selected, or both?

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refined-by | L2-DES-SKILLS-001 | 1 | specs/L2/skills/L2-DES-SKILLS-001-agent-skills-architecture.md | Defines skill package discovery, activation, context integration, trust, and visibility behavior. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-20 | Assistant | Initial | Initial draft with approved L1 refinement. |

---
artifact_id: L1-REQ-APP-008
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Human
last_updated: 2026-05-20
---

# L1-REQ-APP-008 — MCP Capability Integration

## Purpose

Allow users to extend the program with external MCP-provided capabilities.

## Why This Matters

MCP integrations can add powerful external tools and resources, but users need to know what capabilities were added, whether they are healthy, and whether they follow the same safety rules as built-in tools.

## Background / Context

MCP can provide tools, resources, and templates from external servers. Users need discovery, status, safety, and error handling around these capabilities.

## User / Business Requirement

The program must support user-configured MCP integrations as discoverable and controllable product capabilities.

## Real User Scenarios

- A user configures an MCP server and expects its tools and resources to appear as available capabilities.
- A configured MCP server fails to start, and the user needs a clear status message rather than silent missing tools.

## Functional Requirements

- The user must be able to configure MCP servers.
- The user must be able to discover MCP-provided tools, resources, and resource templates.
- The program must show MCP server status and startup errors.
- MCP-provided capabilities must participate in the same safety and approval model as built-in capabilities.

## Non-Functional Requirements

- MCP failures must not make unrelated built-in capabilities unusable.
- MCP capability names and descriptions must be understandable to users before use.

## Acceptance Criteria

- Given a configured MCP server, when discovery succeeds, then the user can see the capabilities it provides.
- Given an MCP server fails to start, when the user inspects integrations, then the failure is visible and actionable.
- Given an MCP-provided tool requires access outside current permissions, when it is requested, then the normal approval and safety flow applies.
- Given an MCP capability disappears after refresh, when the user inspects status, then the program indicates that the capability is no longer available.

## Out of Scope

- The program does not define MCP transport details, protocol implementation, or server lifecycle internals in this L1 requirement.
- This requirement does not guarantee that every third-party MCP server is trustworthy, available, or compatible.

## Open Questions

- Should MCP servers be enabled globally, per workspace, or both?

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refined-by | L2-DES-MCP-001 | 1 | specs/L2/mcp/L2-DES-MCP-001-mcp-integration-architecture.md | Defines MCP configuration, lifecycle, capability discovery, status, safety, and failure behavior. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-20 | Assistant | Initial | Initial draft with approved L1 refinement. |

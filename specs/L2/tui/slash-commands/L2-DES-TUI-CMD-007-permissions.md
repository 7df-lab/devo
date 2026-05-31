---
artifact_id: L2-DES-TUI-CMD-007
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-26
---

# L2-DES-TUI-CMD-007 — Slash Command: /permissions

## Purpose

Define the TUI behavior for `/permissions`, which lets the user choose what the program is allowed to do in the current session.

## Source Requirements

- `L1-REQ-TUI-006` requires discoverable commands for product workflows.
- `L1-REQ-APP-003` requires permission modes, sandboxing, explicit approval for actions outside the current permission boundary, and user-visible approval outcomes.
- `L1-REQ-TOOL-001` requires tool safety, approval, redaction, and bounded output.
- `L2-DES-APP-003` defines approval request/response protocol behavior and session metadata updates.
- `L2-DES-TOOL-001` defines the tool permission policy, sandbox boundary, and approval gates.

## Command Contract

- Command: `/permissions`
- Description: `choose what Devo is allowed to do`
- Parameters: none in the first milestone.
- Mutability: session permission metadata and possibly durable configuration, depending on persistence choice.
- Active-turn availability: allowed only for future work; it must not retroactively authorize already-running tool calls.

## UI Flow

`/permissions` opens a selection popup.

```text
┃ /permissions

> ● default
    auto-approved
    full access
```

Rules:

- The current permission mode is marked with `●`.
- The row focused by Up and Down navigation is marked with `>`.
- If the focused row is also the current permission mode, both markers are shown.
- The popup must summarize the operational effect of each mode.
- Enter applies the focused row; Esc cancels.
- If a tool approval is currently pending, the TUI must distinguish changing the default permission mode from answering that specific approval.

## State And Error Behavior

- Permission changes should be recorded as session metadata changes.
- Permission changes must be submitted to the server as canonical state changes. The TUI displays and requests changes; it does not become the authority for permission or approval decisions.
- The change applies to later tool decisions, not to already-issued provider or tool requests.
- If a mode is blocked by policy, the TUI should show why and keep the existing mode.

## Approval Boundary

`/permissions` changes the session's future permission posture. It is not the same operation as approving a pending tool action.

Rules:

- If a specific approval prompt is pending, the user must answer that prompt through the approval UI, not by changing `/permissions`.
- A permission mode change must not retroactively authorize a tool call that already requested approval.
- A scoped approval response must not be displayed as if it changed the session's default permission mode.
- If permission state is ambiguous or the server rejects the requested mode, the TUI must keep the previous mode visible and show the server-provided reason.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-TUI-006 | 1 | specs/L1/L1-REQ-TUI-006-command-discovery-control.md | Defines command-specific behavior for a discoverable TUI command. |
| related-to | L1-REQ-APP-003 | 1 | specs/L1/L1-REQ-APP-003-safety.md | `/permissions` is the user-facing command for session permission posture and must not bypass approval boundaries. |
| related-to | L1-REQ-TOOL-001 | 1 | specs/L1/L1-REQ-TOOL-001-safety.md | Permissions constrain tool safety and approval behavior. |
| related-to | L2-DES-APP-003 | 2 | specs/L2/app/L2-DES-APP-003-client-server-protocol.md | Permission changes and approval responses are server-owned protocol operations. |
| related-to | L2-DES-TOOL-001 | 1 | specs/L2/tool/L2-DES-TOOL-001-built-in-tool-system.md | Tool permission policy and approval gates define the effect of each permission mode. |
| related-to | L2-DES-TUI-004 | 1 | specs/L2/tui/L2-DES-TUI-004-streaming-transcript-and-state.md | Approval and waiting states must remain visible. |
| related-to | L2-DES-TUI-003 | 1 | specs/L2/tui/L2-DES-TUI-003-composer-and-input-modes.md | Uses shared slash-command discovery and invocation behavior. |
| specified-by | L3-BEH-TUI-004 | 2 | specs/L3/tui/L3-BEH-TUI-004-slash-commands.md | L3 defines consolidated slash command parsing, routing, and permission command behavior. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-23 | Assistant | Initial | Initial `/permissions` command design. |
| 1 | 2026-05-25 | Assistant | Refinement | Added application-safety source requirements and clarified the boundary between permission mode changes and approval responses. |
| 1 | 2026-05-26 | Human | Refinement | Updated permission list marker semantics so `>` marks focus and `●` marks the current enabled permission mode. |

---
artifact_id: L2-DES-TUI-CMD-005
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-27
---

# L2-DES-TUI-CMD-005 — Slash Command: /new

## Purpose

Define the TUI behavior for `/new`, which starts a new chat session.

## Command Contract

- Command: `/new`
- Description: `start a new chat`
- Parameters: none.
- Mutability: prepares a fresh session slot for the next user message.
- Confirmation: no confirmation prompt is shown.
- Active-turn availability: unavailable while the current session has active work; the command is gated by normal busy-state slash-command handling rather than a confirmation flow.

## UI Flow

`/new` immediately prepares a new chat using the current workspace and effective default model configuration. The command does not ask the user to confirm.

```text
┃ /new

<HEADER box>

┃
  BUILD · deepseek-v4-pro high  ↑0[cached 0 0%]  ↓0  ▱▱▱▱▱▱▱▱▱▱  0%  0/950k
```

Rules:

- The current session remains durable and resumable.
- The visible transcript is reset to a new session HEADER box.
- The TUI then waits for the user to send the first message before entering the new session's first turn.
- The new session starts with current workspace metadata and the current effective model and reasoning effort configuration.
- Token counters, active streaming cells, active tool state, and pending tool state reset to the fresh-session baseline.
- If onboarding or model configuration is incomplete, the command should report that model setup is required and direct the user to restart with the onboarding CLI argument defined by `L2-DES-APP-007`.

## State And Error Behavior

- The TUI should request a new session preparation from the background worker.
- The server may defer durable session creation until the first user message is submitted.
- When preparation succeeds, the widget appends or refreshes the HEADER box, clears active turn state, clears pending input cells, resets token counters.
- The command must not delete or overwrite the previous persisted session. It may clear the local visible transcript because the UI is now showing the prepared fresh session.
- If preparation fails, the TUI remains in the current session and shows a concise error.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-TUI-006 | 1 | specs/L1/L1-REQ-TUI-006-command-discovery-control.md | Defines command-specific behavior for a discoverable TUI command. |
| related-to | L1-REQ-CONV-001 | 1 | specs/L1/L1-REQ-CONV-001-session-lifecycle.md | Starting a new chat creates a new session. |
| related-to | L2-DES-APP-003 | 2 | specs/L2/app/L2-DES-APP-003-client-server-protocol.md | Defines session creation behavior. |
| related-to | L2-DES-APP-007 | 1 | specs/L2/app/L2-DES-APP-007-cli-onboarding-entry.md | Defines the onboarding entry path used when required model setup is incomplete. |
| related-to | L2-DES-TUI-003 | 1 | specs/L2/tui/L2-DES-TUI-003-composer-and-input-modes.md | Uses shared slash-command discovery and invocation behavior. |
| specified-by | L3-BEH-TUI-004 | 2 | specs/L3/tui/L3-BEH-TUI-004-slash-commands.md | L3 defines consolidated slash command parsing, routing, and new-session command behavior. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-23 | Assistant | Initial | Initial `/new` command design. |
| 1 | 2026-05-25 | Human | Refinement | Removed confirmation and specified that `/new` adds a HEADER box, then waits for the next user message to enter the new session. |
| 1 | 2026-05-25 | Assistant | Refinement | Updated the flow from immediate session creation with confirmation to no-confirmation new-session preparation. |
| 1 | 2026-05-27 | Human | Refinement | Replaced `/onboard` fallback with guidance to use the onboarding CLI argument when model setup is incomplete. |

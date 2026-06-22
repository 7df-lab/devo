---
artifact_id: L2-DES-TUI-CMD-004
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-26
---

# L2-DES-TUI-CMD-004 — Slash Command: /resume

## Purpose

Define the TUI behavior for `/resume`, which lets the user reopen a saved chat session through the current alternate-screen session browser.

## Command Contract

- Command: `/resume`
- Description: `resume a saved chat`
- Parameters: none.
- Mutability: current interactive client session selection.
- Transcript effect: no model-visible user turn is created by the command itself.
- Rendering mode: alternate-screen session browser, not the below-composer slash-command surface.
- Search: not implemented in the current browser.
- Active-turn confirmation: not implemented in the current browser; command availability is governed by normal slash-command busy-state gating before the browser opens.

## UI Flow

Submitting `/resume` clears any previous resume browser state, marks the browser as loading, sends the host command `session list`, and sets the status message to `Loading sessions`.

The host enters the terminal alternate screen before asking the background worker to list sessions. While the worker request is pending, the full visible area renders the loading view:

```text
Devo Sessions

Resume Session
Loading saved sessions...

Please wait.
```

When sessions are returned, the loading view is replaced by a full-screen session browser:

```text
Devo Sessions

Resume Session  2 / 8 · 33%

    Title                     Session ID                            Updated
    ------------------------  ------------------------------------  -----------------------
    earlier investigation     019db434-c4b4-7c81-ba66-74c58f0fbd60  2026-05-24 22:13:01 UTC
> ● active refactor           019db45d-61ec-7b02-894f-a847b78f7ac3  2026-05-25 04:09:44 UTC
    release notes             019db467-c5ef-7127-9ffd-5e0d9393c3ac  2026-05-25 05:31:10 UTC

↑/↓ select  pgup/pgdn page  home/end jump
enter resume  q back
```

Rules:

- The focused row is marked with `>`.
- The current active session row is marked with `●`.
- If the focused row is also the current active session, both markers are shown.
- The initial selection is the active session when present, otherwise the first row.
- The list shows session title, stable session ID, updated timestamp, and progress through the list.
- Long titles are truncated to fit the title column.
- If the list does not fit, `↑ more` and `↓ more` marker rows indicate hidden rows above or below.
- If no sessions are returned, the browser shows `No saved sessions found.` with the footer `q back`.

Keyboard behavior:

| Key | Behavior |
|---|---|
| `Up` / `Down` | Move selection by one visible row. |
| `PageUp` / `PageDown` | Move selection by one browser page. |
| `Home` / `End` | Jump to first or last session. |
| `Enter` | Resume the selected session when a row is selected. |
| `Esc` / `q` | Close the browser, leave the current session unchanged, and return status to `Ready`. |

`Ctrl+T` full transcript review is disabled while the resume browser is loading or open.

## State And Error Behavior

- The TUI requests the persisted session list through the worker's session-list operation. The worker calls the server session-list API with a five-second timeout and maps each entry into title, session ID, updated timestamp, and active-session marker fields.
- When session listing succeeds, `SessionsListed` clears the host's pending browser flag and opens the browser in the already-active alternate screen.
- When session listing fails or times out, the worker emits a failure event. The TUI clears resume-browser loading state, records the error in the transcript as the normal failure path, and the host leaves alternate screen once no browser is open or pending.
- Pressing `Enter` on a session row clears the visible session UI for switching, sends `SwitchSession { session_id }`, and closes the browser.
- Before dispatching the switch, the widget clears completed history, the active streaming cell, active tool calls, pending tool calls, active text items, and the composer, then sets status to `Resuming session`.
- The host leaves alternate screen before switching, marks `session_switch_pending`, replaces the inline session UI, and asks the worker to resume the selected session.
- The worker resumes by calling the server session-resume API for the selected session ID. On success it emits `SessionSwitched` with restored working directory, optional title, model, reasoning effort selection, reasoning effort, token counters, history items, rich history items, loaded item count, and pending texts.
- On `SessionSwitched`, the widget rebuilds visible history from rich restored items when available, falls back to projected transcript items otherwise, restores pending input queue cells, updates session metadata and token counters, clears busy state, and sets status to `Session switched`.
- Resuming a session must not delete the previously active persisted session.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-TUI-006 | 1 | specs/L1/L1-REQ-TUI-006-command-discovery-control.md | Defines command-specific behavior for a discoverable TUI command. |
| related-to | L1-REQ-CONV-001 | 1 | specs/L1/L1-REQ-CONV-001-session-lifecycle.md | Resuming saved sessions is a session lifecycle workflow. |
| related-to | L2-DES-APP-003 | 2 | specs/L2/app/L2-DES-APP-003-client-server-protocol.md | Defines session listing, opening, and subscription behavior. |
| related-to | L2-DES-TUI-003 | 1 | specs/L2/tui/L2-DES-TUI-003-composer-and-input-modes.md | Uses shared slash-command discovery and invocation behavior. |
| related-to | L2-DES-TUI-005 | 1 | specs/L2/tui/L2-DES-TUI-005-terminal-lifecycle-safety.md | Uses alternate-screen entry and restore behavior for the session browser. |
| specified-by | L3-BEH-TUI-004 | 2 | specs/L3/tui/L3-BEH-TUI-004-slash-commands.md | L3 defines consolidated slash command parsing, routing, and resume command behavior. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-23 | Assistant | Initial | Initial `/resume` command design. |
| 1 | 2026-05-25 | Human | Refinement | Requested alignment with the current `crates/tui` alternate-screen implementation. |
| 1 | 2026-05-25 | Assistant | Refinement | Replaced the stale inline searchable-popup design with the current loading view, alternate-screen session browser, key bindings, worker flow, and session-switch restore behavior. |
| 1 | 2026-05-26 | Human | Refinement | Updated session browser marker semantics so `>` marks focus and `●` marks the current active session. |

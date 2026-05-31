---
artifact_id: L3-BEH-TUI-004
revision: 2
status: Draft
active_baseline: no
---

# L3-BEH-TUI-004 — Slash Command Discovery and Handling

## Purpose

Define the concrete behavior for TUI slash command parsing, popup rendering, server-side routing, and state mutation for all slash commands: `/model`, `/compact`, `/resume`, `/new`, `/status`, `/permissions`, `/clear`, `/goal`, `/btw`, `/exit`, `/theme`.

## Source Design

L2-DES-TUI-CMD-001 through L2-DES-TUI-CMD-012 (Slash Commands), L2-DES-TUI-002 (Modern TUI Shell Layout)

## Behavior Specification

### B1. Slash Command Parsing and Routing

- **Trigger**: User types `/` as the first character in the composer and submits.
- **Preconditions**: Composer is in Default Mode. Text starts with `/`.
- **Algorithm / Flow**:
  1. Parse the composer text: extract `command` (first word after `/`, e.g., `model`, `goal`, `compact`).
  2. Extract arguments: remaining text after the command word.
  3. Look up the command in the slash command registry:
     - Known commands: `model`, `compact`, `resume`, `new`, `status`, `permissions`, `clear`, `goal`, `btw`, `exit`, `theme`.
     - Unknown commands: show an error popup "Unknown command: /<cmd>. Type / to see available commands."
  4. Route to the command handler. Commands that mutate server state (model, permissions, goal) send requests to the server. Client-only commands (theme, exit) handle locally.
  5. Command handler runs. If the command opens a popup, the popup captures keyboard input until dismissed.
- **Postconditions**: The command is executed. Composer is cleared.

### B2. `/model` — Model Selection

- **Trigger**: User submits `/model`.
- **Preconditions**: The client is connected and initialized.
- **Algorithm / Flow**:
  1. Send `model/list` request to server. Receive configured bindings and current model.
  2. Render a selection popup: each row shows binding display name, provider name (muted), model slug (muted). Last row: `Add model...`.
  3. User selects a binding with Enter:
     a. If model supports reasoning: show reasoning effort picker (low, medium, high, xhigh, max, adaptive). User selects or confirms default.
     b. If model does not support reasoning: apply immediately.
     c. Send `model/select` to server with `binding_id` and `reasoning_effort`.
  4. If user selects `Add model...`: enter add-model flow (provider selection → API key → model name → display name → invocation method).
  5. On success: popup dismisses. Top bar shows new model name.
- **Postconditions**: Session model is updated. Next turn uses the new model.

### B3. `/permissions` — Permission Profile Selection

- **Trigger**: User submits `/permissions`.
- **Preconditions**: Client is connected.
- **Algorithm / Flow**:
  1. Render a selection popup with three built-in profiles:
     - `Read-only` — Full disk read, no write, no network.
     - `Workspace` — Full disk read, write within workspace, no network.
     - `Danger Full Access` — Full disk read/write, network enabled.
  2. Show current selection highlighted.
  3. User selects profile: send `config/update` (or dedicated permission change) to server.
  4. Server updates session `permission_profile`. Future tool calls use the new profile.
  5. Active turn is NOT affected (existing tool calls continue under old profile).
- **Postconditions**: Session permission profile is updated for subsequent turns.

### B4. `/goal` — Goal Management

- **Trigger**: User submits `/goal [objective]`.
- **Preconditions**: Session exists.
- **Algorithm / Flow**:
  1. If no objective text (just `/goal`): send `goal/get` to server. Render current goal panel showing status, objective, progress, budgets.
  2. If objective text provided: send `goal/create` with the objective as the goal text. Server creates the goal.
  3. If a goal exists, panel shows action buttons: `[Pause] [Complete] [Cancel] [Clear]`.
  4. User selects action → send corresponding goal mutation to server.
  5. Panel updates on `goal_updated` event.
- **Postconditions**: Goal state is visible and user-controllable from the TUI.

### B5. `/compact` — Manual Compaction

- **Trigger**: User submits `/compact`.
- **Preconditions**: Session has sufficient history to compact.
- **Algorithm / Flow**:
  1. Send a compaction request to the server (or a dedicated protocol method, or via `turn/submit` with compaction intent).
  2. Server runs manual compaction through the context pipeline (`L3-BEH-CORE-005`). Transcript shows `Manual Compaction Started` notice.
  3. On completion: transcript shows `Compaction Done` with summary.
- **Postconditions**: Older transcript turns are summarized. Context pressure is reduced.

### B6. `/status`, `/resume`, `/new`, `/clear`, `/btw`, `/exit`, `/theme`

| Command | Behavior |
|---|---|
| `/status` | Send `execution/inspect` to server. Render active work panel: active turn phase, running tools, pending approvals, background processes. |
| `/resume` | Prompt for session selection. Send `session/list` with `include_archived: false`. Render recent sessions picker. On selection: open and subscribe to the session. |
| `/new` | Confirm dialog: "Start a new session? Current session will remain available." On confirm: create new session via `session/create`. |
| `/clear` | Clear the local transcript display (visual only — does not delete durable history). Re-render from current server state. |
| `/btw` | Start a side conversation in an ephemeral fork of the current session context. The side conversation is runtime-only and must not write session, turn, item, queue, steer, or fork records to durable storage. |
| `/exit` | Graceful disconnect from server (L3-BEH-CLIENT-001 B6). Restore terminal. Exit process. |
| `/theme` | Open theme picker popup. Show available color themes. User selects → persist to client config. Re-render TUI with new theme. |

### B7. Slash Command Discovery

- **Trigger**: User types `/` in empty composer (first character).
- **Preconditions**: Composer is focused. No modal is open.
- **Algorithm / Flow**:
  1. On `/` keypress in empty composer: show a command palette popup overlay.
  2. List all available commands with: command name, description, keyboard shortcut hint (if any).
  3. Commands are shown as a single navigable list using the catalog order from `L2-DES-TUI-003`; do not insert blank row separators between categories.
  4. User can type to filter the list (fuzzy match on command name and description).
  5. User selects with Enter or clicks. The full command text is inserted into the composer. User can add arguments before submitting.
  6. Press Esc to dismiss without inserting.
- **Postconditions**: User discovers available commands. Selected command is ready for argument entry and submission.

## Traceability

| L2 Source | Relationship |
|---|---|
| L2-DES-TUI-CMD-001 through L2-DES-TUI-CMD-012 | specified-by |
| L2-DES-TUI-002 | specified-by |

## Implementation Notes

- Slash command handlers are registered in a `HashMap<&str, SlashCommandHandler>` at client startup.
- Popup rendering uses Ratatui `Clear` and `Paragraph` widgets with centered `Rect` layout.
- Commands that mutate server state send protocol requests and await responses asynchronously. The TUI remains responsive during the wait.
- `/btw` must use an ephemeral fork execution path. It is not lowered into a normal durable `turn/submit` call unless a future explicit promote/copy command is approved.

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-27 | Assistant | Initial | Initial consolidated slash command behavior. |
| 2 | 2026-05-27 | Assistant | Correction | Aligned `/btw` with ephemeral-fork semantics, fixed compaction reference, and removed loose grouped slash-list behavior. |

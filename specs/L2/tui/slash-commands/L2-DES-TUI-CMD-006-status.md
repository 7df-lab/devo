---
artifact_id: L2-DES-TUI-CMD-006
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-23
---

# L2-DES-TUI-CMD-006 — Slash Command: /status

## Purpose

Define the TUI behavior for `/status`, which displays current session configuration, token usage, context pressure, and runtime state.

## Command Contract

- Command: `/status`
- Description: `show current session configuration and token usage`
- Parameters: none in the first milestone.
- Mutability: read-only.
- Active-turn availability: allowed during active work.

## UI Flow

`/status` opens a compact status panel or inserts a bounded status cell.

```text
┃ Status
  model       deepseek-v4-pro
  reasoning   high
  workspace   ~/Desktop/devo
  mode        BUILD
  tokens      ↑420[cached 300 71%] ↓12
  context     ▰▰▱▱▱▱▱▱▱▱ 20% 190k/950k
```

Rules:

- The display must use the same token/cache/context style as the bottom status line.
- Provider credentials must be represented by safe status only, never plaintext secret values.
- Active work should be shown concisely if a turn is running.

## State And Error Behavior

- The command should use server-confirmed session snapshots, usage events, and safe configuration projections.
- Missing or estimated values must be marked instead of invented.
- `/status` must not create a model-visible transcript turn.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-TUI-006 | 1 | specs/L1/L1-REQ-TUI-006-command-discovery-control.md | Defines command-specific behavior for a discoverable TUI command. |
| related-to | L1-REQ-TUI-004 | 1 | specs/L1/L1-REQ-TUI-004-state-visibility.md | Status exposes current session and runtime state. |
| related-to | L2-DES-LLM-003 | 1 | specs/L2/llm/L2-DES-LLM-003-model-usage-observability.md | Defines usage fields and uncertainty handling. |
| related-to | L2-DES-TUI-003 | 1 | specs/L2/tui/L2-DES-TUI-003-composer-and-input-modes.md | Defines bottom status-line fields reused by this command. |
| specified-by | L3-BEH-TUI-004 | 2 | specs/L3/tui/L3-BEH-TUI-004-slash-commands.md | L3 defines consolidated slash command parsing, routing, and status command behavior. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-23 | Assistant | Initial | Initial `/status` command design. |

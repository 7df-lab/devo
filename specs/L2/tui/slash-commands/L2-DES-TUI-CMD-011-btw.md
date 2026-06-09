---
artifact_id: L2-DES-TUI-CMD-011
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-23
---

# L2-DES-TUI-CMD-011 — Slash Command: /btw

## Purpose

Define the TUI behavior for `/btw`, which starts a side conversation inside an ephemeral fork.

## Command Contract

- Command: `/btw`
- Description: `start a side conversation in an ephemeral fork`
- Parameter: free-form text after the command token.
- Mutability: ephemeral runtime state only.
- Active-turn availability: allowed when a session has enough current context to fork.

## UI Flow

Inline command rendering shows the command token in primary color and the parameter hint in muted color.

```text
┃ /btw <side conversation message>

  BUILD · deepseek-v4-pro high  ↑0[cached 0 0%]  ↓0  ▱▱▱▱▱▱▱▱▱▱  0%  0/950k
```

Submission example:

```text
┃ /btw what if we solve this with a smaller parser-only change?

⠋ Working · ⏱ 18s
```

## State And Error Behavior

- The command starts a side conversation using an ephemeral fork of the current session context.
- The side conversation must not write session, turn, item, queue, steer, or fork records to durable storage.
- The side conversation must not mutate the current session transcript, active turn, active context, or persistent configuration.
- The side conversation may use the current visible context as input, but any messages and model responses inside the side conversation are runtime-only.
- Closing or completing the side conversation discards its transcript unless a later explicit command promotes or copies content back into the durable session.
- The TUI must visually distinguish the side conversation from the durable transcript so the user understands it is temporary.
- If the current session context cannot be forked safely, the command should fail with a concise explanation rather than becoming a normal message.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-TUI-006 | 1 | specs/L1/L1-REQ-TUI-006-command-discovery-control.md | Defines command-specific behavior for a discoverable TUI command. |
| related-to | L1-REQ-CONV-004 | 1 | specs/L1/L1-REQ-CONV-004-session-forking.md | `/btw` uses the fork concept for temporary side exploration, while explicitly avoiding durable fork persistence. |
| related-to | L2-DES-CONV-001 | 1 | specs/L2/conv/L2-DES-CONV-001-session-jsonl-data-model.md | Defines the durable session model that `/btw` must not write to while running as an ephemeral fork. |
| related-to | L2-DES-TUI-003 | 1 | specs/L2/tui/L2-DES-TUI-003-composer-and-input-modes.md | Defines inline slash-command coloring and command submission. |
| specified-by | L3-BEH-TUI-004 | 2 | specs/L3/tui/L3-BEH-TUI-004-slash-commands.md | L3 defines consolidated slash command parsing, routing, and ephemeral `/btw` behavior. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-23 | Assistant | Initial | Initial `/btw` command design. |
| 1 | 2026-05-23 | Human | Refinement | Changed `/btw` from active-turn injection to an ephemeral-fork side conversation that is not persisted. |

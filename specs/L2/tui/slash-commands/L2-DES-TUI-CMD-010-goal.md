---
artifact_id: L2-DES-TUI-CMD-010
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-25
---

# L2-DES-TUI-CMD-010 — Slash Command: /goal

## Purpose

Define the TUI behavior for `/goal`, which lets the user create, view, pause, resume, complete, cancel, or clear the session's Ralph Loop goal.

## Command Contract

- Command: `/goal`
- Description: `set or view the goal for a long-running task`
- Parameters: optional free-form objective text. When present, the text following `/goal` is the objective.
- Mutability: goal/session state.
- Active-turn availability: viewing is allowed during active work; mutating actions must be server-serialized and must not rewrite an already-running turn.
- Default budget: none. The first milestone does not prompt for a token, time, or turn budget during goal creation.

## UI Flow

Typing `/goal` without objective text opens the current-goal panel. If no goal exists, it shows an empty state that tells the user to submit `/goal <objective>`.

```text
┃ /goal

  Goal
    status    pursuing
    objective Eliminate the failing parser tests and verify the full parser suite.
    progress  quoted values fixed; escape regression still failing
    budget    none

    [Pause] [Complete] [Cancel] [Clear]

  BUILD · deepseek-v4-pro high  ↑420[cached 300 71%]  ↓12  ▰▰▱▱▱▱▱▱▱▱  20%  190k/950k
```

Typing `/goal <objective>` creates and activates a goal directly. The prompt following `/goal` is the objective; pressing Enter begins execution.

```text
┃ /goal Eliminate the failing parser tests and verify the full parser suite.

  Goal
    objective Eliminate the failing parser tests and verify the full parser suite.
    budget    none
    status    starting
```

Rules:

- `/goal` without parameters opens the current-goal panel or a no-goal empty state.
- `/goal <text>` treats `<text>` as the objective and submits goal creation when the user presses Enter.
- The create path does not open a separate objective editor and does not ask for a budget.
- The first milestone creates goals with no default budget. Optional budget configuration may be added later as an explicit edit/control, not as part of the default create prompt.
- If a non-terminal goal already exists, replacing it requires explicit confirmation.
- The panel must show objective, status, progress, blocker, verification, and budget fields where available. If no budget is configured, the budget field renders as `none` or is omitted in narrow layouts.
- User-owned actions include pause, resume, complete, cancel, clear, create, and replace.
- The model cannot trigger `/goal`; model-originated goal status changes are shown as server events.
- Successful mutations should close the popup or update it in place according to L3 interaction rules.

## Inline Rendering

When the composer recognizes `/goal`, the command token uses the theme primary color and parameter text uses muted color.

```text
┃ /goal <objective for autonomous work>

  BUILD · deepseek-v4-pro high  ↑0[cached 0 0%]  ↓0  ▱▱▱▱▱▱▱▱▱▱  0%  0/950k
```

## State And Error Behavior

- The command uses server-owned goal APIs; the TUI does not mutate local goal state independently.
- Read-only viewing should return the current server-confirmed projection.
- Direct creation with `/goal <objective>` sends only the objective and omitted budget fields unless the user explicitly supplied budget configuration through a later design.
- After successful direct creation, the goal becomes active and the server may begin execution when continuation preconditions permit.
- Mutating actions should pass `expected_goal_id` where the TUI has one, so stale panels do not overwrite newer goal state.
- If the server rejects a stale action, the TUI should refresh the panel and show a concise message.
- If the goal is active and a turn is running, pause/cancel/clear may take effect immediately for future continuation but must not rewrite the current turn's already-built model context.
- If Plan Mode is active, `/goal` remains viewable and user-controllable, but autonomous continuation remains suppressed until Build mode is active.
- `/goal` must not create a model-visible transcript turn.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-TUI-006 | 1 | specs/L1/L1-REQ-TUI-006-command-discovery-control.md | Defines command-specific behavior for a discoverable TUI command. |
| related-to | L1-REQ-GOAL-001 | 1 | specs/L1/L1-REQ-GOAL-001-ralph-loop.md | `/goal` is the TUI control surface for Ralph Loop goals. |
| related-to | L2-DES-GOAL-001 | 1 | specs/L2/goal/L2-DES-GOAL-001-ralph-loop-goals.md | Defines the goal state model, continuation loop, and protocol behavior controlled by this command. |
| related-to | L2-DES-TUI-003 | 1 | specs/L2/tui/L2-DES-TUI-003-composer-and-input-modes.md | Defines slash-command discovery, inline command rendering, and command submission. |
| specified-by | L3-BEH-TUI-004 | 2 | specs/L3/tui/L3-BEH-TUI-004-slash-commands.md | L3 defines consolidated slash command parsing, routing, and goal command behavior. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-23 | Assistant | Initial | Initial `/goal` command design. |
| 1 | 2026-05-25 | Human | Refinement | Set default goal creation to no budget and made `/goal <objective>` submit the objective directly on Enter. |
| 1 | 2026-05-25 | Assistant | Refinement | Removed the default create panel budget prompt and documented direct objective submission. |

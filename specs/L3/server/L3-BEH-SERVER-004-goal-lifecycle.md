---
artifact_id: L3-BEH-SERVER-004
revision: 2
status: Draft
active_baseline: no
---

# L3-BEH-SERVER-004 — Goal Lifecycle and Autonomous Continuation

## Purpose

Define the concrete behavior for Ralph Loop goal state management, user-controlled mutations, budget tracking, autonomous continuation turns, and model-facing goal updates as specified by L2-DES-GOAL-001.

## Source Design

L2-DES-GOAL-001 (Ralph Loop Goals), L2-DES-AGENT-001 (Execution Engine), L2-DES-CONTEXT-001 (Context Assembly)

## Behavior Specification

### B1. Goal Creation

- **Trigger**: User sends `goal/create` or the `/goal` command with an objective.
- **Preconditions**: Session exists. No non-terminal goal is active, or `replace_existing` is true.
- **Algorithm / Flow**:
  1. If a non-terminal goal exists and `replace_existing` is false: error with `GoalAlreadyExists` and the existing goal_id.
  2. Generate `GoalId`. Create `Goal` struct: `goal_id`, `session_id`, `objective`, `status: Active`, optional budget fields, `tokens_used: 0`, `time_used_seconds: 0`, `turns_used: 0`, `progress_summary: None`, `blocker_summary: None`, `verification_summary: None`, `created_at`, `updated_at`.
  3. If no budget was supplied, leave all budget fields unset. Do not infer a default token, time, or turn budget.
  4. If `replace_existing`: append one `goal_replaced` durable record that ends the prior current goal projection and creates the replacement `goal_id`. Do not mutate the previous goal record in place.
  5. If this is the first goal: append `goal_created`.
  6. Apply the event to the rebuildable live projection only after the JSONL record is durable.
  7. Broadcast `goal_updated` event.
- **Postconditions**: The session has one active goal. Old goal (if any) is auditable.

### B2. Goal Mutation (Pause, Resume, Complete, Cancel, Clear)

- **Trigger**: User sends `goal/pause`, `goal/resume`, `goal/complete`, `goal/cancel`, or `goal/clear`.
- **Preconditions**: The goal exists. `expected_goal_id` matches current goal (stale-check).
- **Algorithm / Flow**:
  1. Validate `expected_goal_id` if provided. If mismatched → stale-state error with current goal_id.
  2. Apply the requested transition:
     - `pause`: active → `Paused`. Record reason.
     - `resume`: paused/blocked → `Active` if configured budgets permit another continuation. If a configured budget is already exhausted, write `budget_limited` instead of resuming.
     - `complete`: any non-terminal → `Complete`. Require `verification_summary`.
     - `cancel`: any non-terminal → `Canceled`. Require reason.
     - `clear`: terminal or canceled goal → remove from normal active projection. Keep audit records. Append `goal_cleared`.
  3. Persist `goal_status_changed` record.
  4. Broadcast `goal_updated` event.
  5. If pausing/canceling while a goal continuation turn is active: the turn may finish naturally, but no NEW continuation turns will start. If the user explicitly wants to stop the active turn, they must use `turn/interrupt`.
- **Postconditions**: Goal state is updated. Future continuation eligibility reflects the new state.

### B3. Model-Facing Goal Update Tool

- **Trigger**: Model calls the goal update tool (narrow tool, distinct from user-facing goal commands).
- **Preconditions**: Session has an active goal. The model's context includes the goal state.
- **Algorithm / Flow**:
  1. Validate `expected_goal_id`. If stale → no-op result with factual stale-state summary.
  2. Allowed operations only:
     - Mark `complete`: set `verification_summary` with evidence references. Transition goal to `Complete`. Append `goal_status_changed`.
     - Mark `blocked`: set `blocker_summary` describing what input/state change is needed. Transition goal to `Blocked`. Append `goal_status_changed`.
  3. Disallowed operations (return error, do not apply): create goal, replace goal, edit objective, change budget, pause, resume, clear, cancel.
  4. Broadcast `goal_updated` from the model's action.
- **Postconditions**: Goal state reflects model-reported completion or blocker. Client-visible events are emitted.
- **Error Handling**: Model attempts disallowed operation → structured error result, no state change. Stale goal_id → no-op result.

### B4. Budget Tracking

- **Trigger**: Turn start, tool completion, model-facing goal tool completion, turn completion, interruption, and external user goal mutation.
- **Preconditions**: Session has a current non-terminal goal. Accounting may run even when no explicit budget is configured.
- **Algorithm / Flow**:
  1. At turn start, capture runtime baselines for normalized model usage, wall-clock time, and goal-driven turn count.
  2. At each accounting point, compute deltas since the last accounting point:
     - `delta_tokens = non_cached_input_tokens + output_tokens`.
     - Exclude cached input tokens.
     - Do not double-count reasoning tokens; rely on provider usage normalization to state whether reasoning tokens are included in output tokens.
     - `delta_time_seconds = now - last_accounted_time`.
     - `delta_turns = 1` only when counting a newly launched goal continuation turn, not every user or Plan Mode turn.
  3. Append `goal_budget_accounted` with the delta and resulting totals.
  4. Apply the same event to the live projection atomically with any status transition:
     - If a configured token, time, or turn budget is reached from `active`, append `goal_status_changed` with status `budget_limited`.
     - If no explicit budget exists, continue recording usage without transitioning to `budget_limited`.
  5. If the status becomes `budget_limited`, broadcast `goal/budget/limited` and `goal/updated`. No more autonomous continuation turns may start.
- **Postconditions**: Budget counters are accurate. Budget-exhausted goals stop continuation.

### B5. Autonomous Continuation

- **Trigger**: A turn completes, session is idle, and an active goal is eligible for continuation.
- **Preconditions**: Goal status is `Active`. Budget not exceeded. `interaction_mode` is not `plan`. No active turn and no pending queued items.
- **Algorithm / Flow**:
  1. Run the required pre-check:
     - goals feature enabled,
     - current goal status is `Active`,
     - interaction mode is not Plan,
     - no active turn,
     - no queued user work has priority,
     - no pending approval or question prompt,
     - configured budgets permit another continuation,
     - previous continuation was not suppressed for no useful work.
  2. Acquire the continuation lock and reserve the active-turn slot before launching work.
  3. Re-read the goal projection after reservation. Verify the same `goal_id`, status `Active`, mode, budget, and idle-session conditions still hold.
  4. If still eligible, create hidden continuation context from goal state:
     - Include objective, progress summary, blocker summary, verification status.
     - Include budget reminder only for configured budgets; do not fabricate a budget.
     - Instruction: continue working toward the goal. If complete, use the goal update tool to mark verified completion.
  5. Record `goal_context_snapshot_recorded` before the model invocation or store a stable content reference to the exact hidden context.
  6. Submit as a `GoalContinuation` turn through the normal execution engine.
  7. Broadcast `goal/continuation/started` alongside `turn_started`.
  8. If the continuation completes without tool calls, verification, progress update, or useful assistant output, set the runtime suppression flag and report that the goal needs user input or review.
- **Postconditions**: The agent continues working autonomously. Clients can see the continuation turn in the transcript.
- **Edge Cases**: User submits input during continuation → the continuation completes (or is interrupted), then the user's turn runs. Multiple rapid continuations → rate-limited by a cooldown (default 1 second between continuations). Plan Mode active → suppress continuation entirely.

### B6. Goal Context for Continuation

- **Trigger**: Context assembly for a goal continuation turn.
- **Preconditions**: The turn is `GoalContinuation` kind.
- **Algorithm / Flow**:
  1. Construct hidden goal context through context assembly (`L3-BEH-CORE-005`) from current goal state.
  2. Escape the user-owned objective before placing it inside structured tags such as `<untrusted_objective>`.
  3. The model sees: `[immutable prefix] [persona/mode instructions] [hidden goal context] [consolidated change signal if any] [current turn driver]`.
  4. Hidden goal context is not recorded as a visible transcript item.
  5. The continuation turn itself is durable and visible through normal turn/item records, but its model-driving goal prompt remains metadata-derived context, not a user-authored transcript message.
- **Postconditions**: The model is guided by the goal. The user's transcript shows the continuation turn and its outputs.

## Traceability

| L2 Source | Relationship |
|---|---|
| L2-DES-GOAL-001 | specified-by |
| L2-DES-AGENT-001 | specified-by |
| L2-DES-CONTEXT-001 | specified-by |
| L2-DES-APP-003 | specified-by |

## Implementation Notes

- Goal state is per-session, stored in memory and persisted to JSONL.
- JSONL is the source of truth. If SQLite or another live projection is used for atomic accounting, it must be rebuildable from JSONL and must not become the only durable copy of goal state.
- Budget checks happen incrementally at the accounting points in B4, not only at turn completion.
- Continuation cooldown prevents tight loops: after a continuation turn completes, wait `goal_continuation_cooldown_ms` (default 1000) before starting the next.
- The goal update tool is registered in the tool registry with `tool_category: goal_status` to distinguish it from user-facing goal operations.

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-27 | Assistant | Initial | Initial goal lifecycle and autonomous continuation behavior. |
| 2 | 2026-05-27 | Assistant | Correction | Aligned accounting, hidden context, continuation reservation/recheck, no-default-budget behavior, and JSONL source-of-truth rules with L2. |

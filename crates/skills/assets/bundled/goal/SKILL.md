---
name: goal
description: Manage the persistent thread goal from the Python REPL. Use to read goal status and budget usage, to start a goal when the user explicitly asks for one, to pause, resume, or cancel a goal when requested, or to mark the active goal complete once its objective is fully achieved.
---

# Goal

The thread goal is a persistent objective the harness keeps re-prompting you to
pursue across turns until it is complete. Goal state (status, token budget,
usage accounting) lives in the host; this skill is the kernel-side interface to
it. Call it directly from the Python REPL:

```python
await goal.get()
await goal.create("ship the release notes")
await goal.pause()
await goal.resume()
# If the user cancels the objective: await goal.cancel()
await goal.complete()
```

## API

- `await goal.get()` — current goal as a dict: `goal` (or `None` when no goal
  is set), `remaining_tokens`, and `completion_budget_report`. The `goal` dict
  carries `objective`, `status`, `token_budget`, `tokens_used`,
  `time_used_seconds`, and timestamps.
- `await goal.create(objective, token_budget=None)` — start a new active goal.
  Fails while a goal is still pending (active, paused, or budget-limited); a
  completed or errored goal is replaced by the new one. Only create a goal when
  the user or system/developer instructions explicitly ask for a persistent
  long-running goal; do not infer goals from ordinary tasks. Set `token_budget`
  only when an explicit token budget is requested.
- `await goal.complete()` — mark the existing goal achieved. Use only when the
  objective has actually been achieved and no required work remains; do not
  call it merely because the budget is nearly exhausted or because you are
  stopping work. When the result includes a `completion_budget_report`, report
  that final usage to the user.

- `await goal.pause()` — stop automatic pursuit and retain the objective.
- `await goal.resume()` — resume a paused goal without resetting usage or budget.
- `await goal.cancel()` — stop pursuing an unwanted goal without claiming success.

These calls return `status: "ok"` and the updated goal, or `status: "error"`
with an error message. Check the result before announcing success. Pausing or
canceling from Python lets the current call finish; it prevents further goal
continuations rather than aborting its own tool execution.

## Rules

- Use pause, resume, and cancel when requested by the user or system/developer
  instructions. Cancel is not completion; do not report the objective achieved.
- Clear and budget-limiting remain host-controlled; no Python API exposes them.
- When an active goal is actually complete, call `await goal.complete()`; do
  not merely say it is done — the harness keeps continuing the goal until the
  completion call arrives.

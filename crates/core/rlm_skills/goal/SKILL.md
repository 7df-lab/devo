---
name: goal
description: Manage the persistent thread goal from the Python REPL. Use to read goal status and budget usage, to start a goal when the user explicitly asks for one, or to mark the active goal complete once its objective is fully achieved.
---

# Goal

The thread goal is a persistent objective the harness keeps re-prompting across
turns until complete. Goal state lives in the host (`session/goal/*`); this
skill is the kernel-side interface.

```python
await goal.get()
await goal.create("ship the release notes", token_budget=200000)
await goal.complete()
```

## API

- `await goal.get()` — current goal, remaining tokens, budget report.
- `await goal.create(objective, token_budget=None)` — start an active goal (only when explicitly requested).
- `await goal.complete()` — mark the objective achieved.

## Rules

- Pause / resume / clear / budget-limit are user/host controlled.
- Call `complete()` when the objective is actually done — do not only say it is done.

**Status (P1b):** Python package stub — `host_request` lands in P2.

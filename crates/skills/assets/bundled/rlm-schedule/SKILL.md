---
name: rlm-schedule
description: Schedule and manage recurring checks for the current agent session from the Python REPL. Use for agent-owned checks, not for the user's own /schedule command.
---

# Scheduled Checks

A scheduled check sends a recurring prompt to the current session. Devo's
server scheduler delivers it at the configured interval; it does not keep a
Python process running. Loading this skill does not create a schedule.

Use a non-empty label for every check. An unlabeled check uses the same slot as
the user's `/schedule`, so creating one can replace their schedule. Only
manage labeled checks you created with this skill.

Call directly from the Python REPL:

```python
import rlm_schedule

await rlm_schedule.create("check test progress", interval="5m", label="tests")
await rlm_schedule.create("watch build", label="build", delivery_mode="follow_up")
await rlm_schedule.list()
await rlm_schedule.update("job-id", status="pause")
await rlm_schedule.delete("job-id")
```

## API

- `await rlm_schedule.list(include_inactive=False)` — return this session's recurring
  checks under the `schedules` key. Includes active and paused entries by default.
  Ignore the user's unlabeled check.
- `await rlm_schedule.create(instruction, interval=None, label=None,
  delivery_mode=None)` — create a recurring check for this session. The default
  interval is every 5 minutes. Use labels to distinguish checks.
- `await rlm_schedule.update(id, instruction=None, interval=None, label=None,
  status=None, delivery_mode=None)` — update a check by ID. `status` may be
  `"pause"` or `"resume"`.
- `await rlm_schedule.delete(id)` — cancel a check by ID.

## Delivery mode

- `steer` (default): deliver at the next safe model boundary in the current turn;
  it does not cancel an in-flight model response.
- `follow_up`: wait for the current turn to finish before delivering the check.

## Rules

- Use this only for agent-owned recurring checks and long-running task coordination.
- Use `/schedule` to configure the user's own recurring check and `/schedules`
  to view and manage scheduled tasks. Do not use this skill in their place.
- Keep instructions specific so each recurring turn knows what to inspect or continue.

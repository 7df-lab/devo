---
name: refine
description: Continual harness refinement (prompt notes, memory, skills, subagents). Schedule `await refine.run(...)` / `await refine.status()`; apply happens at turn boundary.
---

# Refine skill

Schedule continual harness refinement from the Python REPL. Refinement never runs mid-cell — the host applies pending edits when the turn ends.

```python
await refine.status()
await refine.run(instructions="Prefer local memory for this session's blockers")
await refine.run(instructions="Promote durable preference", global_=True)
```

Harness CRUD for memories and related entries lives on `rlm.harness` (requires
`RLM_SESSION_DIR` / `RLM_HARNESS_STATE_DIR` from the host):

```python
rlm.harness.create_memory(title="...", content="...")
rlm.harness.overview()
```

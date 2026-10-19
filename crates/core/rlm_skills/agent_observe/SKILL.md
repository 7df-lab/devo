---
name: agent_observe
description: Observe nuclear-family agents (self, parent, siblings, children) from the Python REPL. Use to list related sessions, inspect one agent, or read recent message previews.
---

# Agent observe

Nuclear-family observation for multi-agent work. State is computed on the host;
this skill is the kernel-side interface.

```python
await agent_observe.list_agents()
await agent_observe.get_agent("child-name-or-session-id")
await agent_observe.recent_messages("child-name-or-session-id", limit=8)
# Short aliases also work: list / get / recent
```

## API

- `await agent_observe.list_agents()` — roster: current + family agents.
- `await agent_observe.get_agent(target)` — one matching family agent.
- `await agent_observe.recent_messages(target, limit=8, max_chars=800)` — recent message previews.

## Rules

- Targets must resolve to exactly one family agent.
- Heartbeats / non-family agents are out of scope for this skill.

---
name: agent-message
description: Message an agent's parent, siblings, or direct children through the daemon. Discover reachable agents with agent_observe.list_agents, then send direct text without spoofing sender identity.
---

# Agent Message

Send direct messages within the current agent's nuclear family through the
local daemon: parent, siblings, and direct children only. Roots are siblings
of each other only when they share a parent worker; a root has no parent to
message. The daemon derives your sender identity from the current session; do
not try to include a `from` field.

Call directly from the kernel:

```python
roster = await agent_observe.list_agents()
child = next(
    (
        item
        for item in roster.get("agents", [])
        if item.get("relationship") == "child"
    ),
    None,
)
if child is not None:
    receipt = await agent_message.send(
        "Please inspect the latest result.",
        receiver_role="child",
        receiver_name=child.get("sessionName") or child["sessionId"],
    )
```

## API

- `await agent_observe.list_agents()` (agent-observe skill) is the roster: it
  lists the parent, siblings, and children this skill can reach, active or not,
  with the `relationship` and `sessionName` that `send` takes as
  `receiver_role` and `receiver_name`.
- `await agent_message.send(message, receiver_role="parent" | "sibling" | "child", receiver_name=None)` — sends one direct
  text message to one family member. Sending to an inactive or idle completed
  subagent wakes it and starts an ordinary follow-up turn in that same session
  and context.
  The child remains available only until its parent session closes. The daemon
  resolves `receiver_role` within the current agent family; `receiver_name` is
  required for siblings and children and omitted for the unique parent.
  `send("all", message)` broadcasts only to the family roster and returns
  `{receipts: [...]}` in roster order; successful entries are ordinary receipts
  and failed entries contain the target id and a short `error`. One failed delivery
  does not reject successful deliveries. Messages always use steering delivery so
  a busy target sees them during its active run. Returns a receipt with a
  `deliveryStatus` field: `"delivered"` means the message reached an idle target's
  context; `"queued"` means a steering message was accepted and will deliver when
  the target's current work allows (`send` does not block waiting for that).
  Delivered receipts carry `deliveredAt`, queued receipts carry `queuedAt`.

## Safety

- Do not delete a child immediately after `send`: delivered follow-ups may still
  be running and queued receipts have not run yet. Wait until observation shows
  the child is idle and its context is no longer needed before closing it.
- Reach is limited to parent, siblings, and direct children; relay through an
  intermediate child instead of messaging grandchildren or cousins directly.
- Sender identity is daemon-derived and cannot be spoofed from Python.
- The daemon enforces message size, rate, and pending-queue limits before
  accepting delivery.

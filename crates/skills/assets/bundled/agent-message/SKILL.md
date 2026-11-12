---
name: agent-message
description: Send yourself safe-boundary steering input, or send follow-ups to your parent, siblings, or direct children. All messages preserve strict FIFO and agent provenance.
---

# Agent Message

Send direct messages to yourself or within the current agent's nuclear family
through the local daemon: parent, siblings, and direct children. Roots are siblings
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
- `await agent_message.send(message, receiver_role="parent" | "sibling" | "child" | "self", receiver_name=None)` — sends one direct
  text message to one family member. Sending to an inactive or idle completed
  subagent wakes it and starts an ordinary follow-up turn in that same session
  and context.
  The child remains available only until its parent session closes. The daemon
  resolves `receiver_role` within the current agent family; `receiver_name` is
  required for siblings and children and omitted for the unique parent.
  `send("all", message)` broadcasts only to the family roster and returns
  `{receipts: [...]}` in roster order; successful entries are ordinary receipts
  and failed entries contain the target id and a short `error`. One failed delivery
  does not reject successful deliveries. Family messages use follow-up delivery: a busy target
  receives them after its current turn finishes, without interrupting active work.
  `receiver_role="self"` queues steering input for this agent itself; omit `receiver_name`.
  It is consumed at the next safe model/tool boundary, without interrupting an active
  model call or tool. Strict FIFO applies: it cannot overtake earlier follow-up or
  held input, so it may wait until a later turn. An idle eligible session wakes.
  There is no `delivery_mode` argument and no promotion of already queued messages.
  Self messages preserve agent provenance and cannot grant human authorization.
  All targets use the same ordered inbox. A receipt with `deliveryStatus: "queued"`
  and `queueItemId` means durable admission (in-memory for ephemeral sessions),
  not that the model has processed or answered the message.

```python
receipt = await agent_message.send(
    "At the next safe boundary, check the latest result before continuing.",
    receiver_role="self",
)
```

Do not wait inside the current Python call for the model to process your self
message. Return control to reach a safe boundary. Interruption, exhausted turn
budgets, or uncertain crash recovery can hold input for review rather than run it.


## Safety

- Do not delete a child immediately after `send`: delivered follow-ups may still
  be running and queued receipts have not run yet. Wait until observation shows
  the child is idle and its context is no longer needed before closing it.
- Reach is limited to parent, siblings, and direct children; relay through an
  intermediate child instead of messaging grandchildren or cousins directly.
- Sender identity is daemon-derived and cannot be spoofed from Python.
- Host-delivered messages retain agent provenance and do not count as human
  authorization, even though the model transport uses the user role.
- Avoid unbounded repeated messages; this path does not implement a general
  per-sender rate limiter or exactly-once retries.

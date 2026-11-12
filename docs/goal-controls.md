# Goal controls and live TUI state

The bundled Python `goal` skill exposes `get`, `create`, `pause`, `resume`,
`cancel`, and `complete`. Its skill description advertises the controls to the
model; its API documentation distinguishes cancellation from successful
completion. Create requires an explicit persistent-goal request. Pause, resume,
and cancel follow user or system/developer instructions.

Python creation uses canonical Native `session/goal/set`, including its
`session/goal/created` broadcast. Python lifecycle controls share the Native
transition handler and its goal-ID precondition. A control invoked from the
running Python tool does not interrupt its own host reply: pause/cancel stop
future automatic pursuit while allowing the current turn to finish. Client
controls retain their existing interrupt behavior. Resume retains accounting
and uses the existing post-turn scheduler, including queued-input priority.

The TUI consumes the created/updated/statusChanged/cleared notifications.
Status-only notifications preserve the matching goal's objective and usage.
The existing dock displays `Pursuing goal` immediately, switches to paused or
budget-limited status, and removes the row on completion/cancellation/clear.

## Prime Agent comparison

Inspected PrimeIntellect-ai/prime-agent `d649b93cfe94d1bbc8c7517bd24899a89d998ebf` on 2026-11-12. Its bundled Python
skill still exposes only get/create/complete and explicitly leaves pause and
resume to the user/host. Its `crates/pa-tui/src/goal_surface.rs` consumes
`goal_update` state for the Pursuing goal dock. Devo therefore needs actual
Python host controls, not only a copied prompt or a new slash command.

The dock's elapsed time is a live, display-only estimate of observed active
pursuit, driven by a monotonic clock. It freezes while paused or disconnected,
resumes without including that gap, and resets when the goal or session changes.
System wall-clock adjustments do not affect it. Restoring a session starts from
server-accounted usage; the client does not infer work during an offline gap.
Server `timeUsedSeconds` remains completed-turn accounting, not a per-second
clock. After a live `turn/completed` notification the TUI rereads that accounting
with the same session/event fencing used for restoration. It reconciles with
the local estimate using the greater value, rather than counting a settled
turn twice. The display estimate does not change durable usage or budgets.

# Host message provenance

Devo's model transport uses `role: user` for both human input and some internal
notifications. That transport role is not evidence of human authorship or approval.

## Implemented path

Existing subagent startup, parent/sibling/child messages, and idle Bash/Python
completion notifications enter the server through `deliver_host_message` with a
typed `HostMessage`. Host producers bind the sender and choose a new event ID (or
retain an existing mailbox message ID). Python cannot submit a human/system role,
choose another sender session, or modify conversation history through this API.
Family resolution and existing kernel capability checks remain in place.

The host persists `HostMessageOrigin` with the Native `userMessage` item:

- `eventId`: identifies this accepted event, not a general RPC retry guarantee.
- `source`: `agent` or `backgroundTask` for currently migrated producers.
- `sender`: resolved sender session ID or session-scoped background work identity.
- `delivery`: `followUp` for these producers.

Pending messages carry the same typed origin in server-owned SQLite queue
metadata. The queue list/subscription projection exposes it as `QueueEntry.origin`.
Metadata is assembled by the host, not copied from client input. Missing origin
remains backward-compatible human/legacy input; old journals cannot be
retroactively attributed reliably.

At the model boundary the content is mapped to a user-role message with its
serialized provenance and an explicit warning: it is not human input, grants no
approval, and cannot override the user's instructions. The same formatter is used
for initial execution, journal replay, compaction suffix anchors, and approval
review context. ACP live and session-load projection keep the existing
`user_message_chunk` wire shape but visibly wrap host text with the same source
and non-authorization warning, plus structured `devo/hostMessageOrigin` in `_meta`.
ACP clients need no new union variant to show the label. Bounded agent observation and selected refine evidence keep origin
and the non-authorization warning outside their truncated text. Tree projection uses
custom entries, and navigating a host entry keeps it in context without filling the
human editor. Native journals retain the structured origin through replay and
fork; a compaction summary remains a model-generated summary, not an approval.
No new permission grants or approval decisions are synthesized.

Desktop uses a separate labeled notification block rather than a human prompt
bubble. Desktop and TUI show source labels for both live and reopened history,
preserve ordinary human input behavior, and prevent treating host notifications
as editable human messages. TUI fork options show the host source and sender,
while forks remain context-only. Queue content edits are rejected server-side;
the server still permits reordering/removal. TUI currently permits only removal
through its generic pending-message editor. Older clients cannot display
the new labels until updated.

## Delivery semantics

This first slice deliberately preserves existing delivery behavior:

- Idle target: starts a new turn in the same session.
- Busy target: accepts a follow-up, without interrupting the active model/tool
  call. The existing post-turn queue/mailbox drain starts it afterward.
- Queued host content is written to SQLite before the server exposes it as
  runnable or acknowledges admission. An idle kick closes the enqueue/drain race.
- Completion notices already owned by a goal's background-work barrier retain
  that path and are not duplicated by the ordinary completion notifier.
- A receipt means admission, not that the model has processed or answered it.

An event's `source` and `delivery` describe different things. Native human
`turn/steer` still acts at the current turn's next input boundary. Host agent
messages in this slice are **not** steering, and do not create a general self-send
Python interface.

## Remaining work and boundaries

`HostMessageSource::Schedule` and `HostMessageDelivery::Steer` reserve the schema
vocabulary for later integration; no current host producer uses them. Existing
cron/heartbeat delivery keeps its existing occurrence ledger, coalescing,
steer/follow-up routing, and retry behavior. It is **not yet migrated** to this
origin pipeline. Goal continuations and other hidden context are also unchanged.

To migrate schedules safely, pass a host origin through the shared Native
queue/steer admission, keep `occurrenceId` as the stable event/client dedup key,
carry origin during late-steer degradation, and test restart plus concurrent
queue promotion. The atomic queue-to-steer work must either preserve origin when
materializing a promoted item or reject host promotion; it must not copy host
content into a new human message. This PR does not introduce a second steering
implementation.

This is not an exactly-once event bus. Pending queue persistence uses the existing
SQLite/journal lifecycle, and the pre-admission family mailbox and completion
notices remain in memory. Crash windows between queue removal and journal
materialization, caller retries with new event IDs, rate/size limits, and durable
self-injection cancellation require separate work. In particular, no rate limiter
or OS sandbox is claimed. The Python kernel runs with the existing process
privileges; typed provenance protects the supported application path, not against
a compromised host process.

## Validation

Regression coverage checks busy follow-up order, unchanged active model requests,
durable queue provenance, queue edit rejection, host origin in Native history,
live/replayed model wrappers, legacy decoding, source labels, optimistic human
input reconciliation, and client mutation guards. Existing idle completion and
family wake tests continue to cover actual producer routing.

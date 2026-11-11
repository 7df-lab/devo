# Agent input ordering in the terminal

Each agent owns one admission-ordered input queue. Delivery mode controls when an entry is eligible, not its priority:

- **Enter** submits steering input. During a run it becomes eligible at the next safe model-request boundary, after the current model response and awaited tools finish.
- **Alt+Enter** submits follow-up input, eligible after the current turn finishes.
- A follow-up at the head blocks every later entry, including steering. The terminal shows these steering entries as **waiting behind earlier input**. A later steering message never silently overtakes earlier work.

The numbered pending list and queue browsing use server admission order, not separate steering and follow-up lists. Server acknowledgments mean admission, not model consumption: accepted messages remain visible until the server consumes or removes them. Editing an entry preserves its place. Reordering and changing delivery mode through the queue editor are not supported; neither operation silently removes and resubmits a message.

Background completions retain their source identity and target agent. Control operations such as cancellation and approval replies are not queued ordinary input. A paused approval must still be answered directly.

The authoritative queue is restored from the server after reconnecting. A late snapshot cannot overwrite a more recent queue notification. Identical message text does not identify an entry: editing/removal uses the server queue ID.

## Recovery-held entries

A held entry is explicitly labeled **Held · review required**. Holds can follow interruption, recovery, or a turn limit. The entry may already have partially executed. It blocks later input and is never retried automatically. Browse the entry and submit empty text to remove it safely from the queue. The TUI does not currently offer a retry button: after reviewing the previous turn and possible external effects, an explicit Native `session/queue/retry` request can release the hold, but is rejected while the applicable turn limit is exhausted. A retry can repeat external side effects; do not treat it as an exactly-once guarantee.

Clearing the queue removes the listed IDs at the server. Entries already consumed are harmless removal races. Other failures are reported and the remaining authoritative queue is kept visible; the client never hides durable messages by merely clearing its local preview. New messages admitted during a clear can remain and are shown after refresh.

Queue previews may be shortened for display, but text-only editing starts from the complete `input` payload, including all lines. Entries containing images, other non-text parts, host-origin content, or missing full input are read-only in the queue editor. The TUI explains this restriction rather than silently replacing them with preview text and losing attachments. Such entries can still be removed explicitly.

## Durability boundary

For persisted sessions, successful admission commits an inbox entry and its deduplication receipt before acknowledging it. Repeated delivery of the same source event does not create a second entry, including after the first entry was consumed. Ephemeral sessions retain this protection only for the lifetime of the runtime.

This is inbox durability, not a transaction covering every external producer. Detached completion admission failures are retried while the owning runtime and work ticket remain alive. A process crash before admission can still lose a producer's in-memory completion; this change does not add a durable outbox for every shell or Python task, nor does it promise exactly-once external effects.

Strict FIFO deliberately waits behind an ineligible head. An indefinitely running request or awaited tool can therefore delay a follow-up and later steering input. Use the explicit interrupt/cancel controls when appropriate; the queue does not silently bypass that head. Queue capacity and receipt retention limits are not introduced by this change.

The dedicated queue and steering admission APIs return `AlreadyAccepted` when a known receipt is retried. A repeated `turn/start` after process restart may instead return a safe conflict/error rather than reconstructing the original turn response; it must not replay the consumed input. A host notification arriving after explicit interruption, or after the owner's turn limit, is kept held rather than silently restarting the agent. Turn-limit holds cannot be released through queue retry while that limit applies.

The idle-interrupt input suspension flag is runtime-local; it is not a persistent scheduler pause across process restarts. Entries already marked held are persisted normally. To stop recurring schedules durably, pause or remove the schedule itself.

# Atomic queued steering

`session/queue/steer` transfers one existing `(sessionId, queueItemId)` to an
expected active regular turn. It does not reconstruct the input from a preview,
re-resolve its attachments, or call `turn/steer` followed by queue removal.

The complete resolved pending item, including input parts, prompts, images,
metadata and identity, moves under the session admission gate and queue mutex.
SQLite records the transfer and an admission receipt in one transaction before
memory changes. Concurrent editing, removal and queue draining share the
admission gate. Turn finalization closes transfers before terminal I/O, rather
than relying on the actor's later `MergeTurn` status change.
An explicit `turn/resume` reopens admission for the recovered execution attempt,
which retains the original turn ID. Existing queue admission receipts still
prevent previously accepted identities from being injected again.

## Outcomes and retries

- `accepted` contains the original queue and turn IDs. Repeating the same queue
  identity returns that receipt even with no cached queue, a different current
  turn, a remounted composer, or a lost response. It does not replay the input.
- `keptQueued` contains the original entry when the expected turn is stale,
  nonregular, parked for approval, or the input requires recovery review.
- A missing entry without an admission receipt fails explicitly. Unsupported
  servers fail without a two-RPC fallback.

An admission receipt is not evidence that the model consumed the input or that
external effects completed. The client refreshes the authoritative queue even
when replaying a receipt, because the same entry can have returned to the queue.
Receipt storage is retained for the session lifetime and is removed with the
session. Ephemeral sessions deduplicate only for the live session lifetime.
The guarantee assumes the same persistent database; deleting/replacing that
store removes its receipts.

## Completion and crash recovery

At ordinary completion, unconsumed steering remains queued and consumed rows
are removed in one settlement transaction. Known queued order is preserved,
including DB-first enqueues that have not reached the in-memory queue yet.

After a crash, remaining receipt-backed steering rows become queued entries with
`recoveryRequired: true`. A user bubble, text match or stored token is not used
to infer consumption. Held entries never auto-drain, and already queued FIFO
input cannot skip a held head. A new explicit input on an idle session keeps
the existing Native start-new-turn behavior; it does not release or execute
the held input. Editing or reordering cannot release a hold.

The UI explains that previous effects may have occurred. The user can discard
or explicitly retry after reviewing the conversation. `session/queue/retry`
creates a fresh queue identity and retains the old receipt; the retry request
itself is never automatically replayed. A lost retry response is reconciled by
queue refresh. Removing the held head wakes ordinary input behind it.

Settlement storage failures hold affected input conservatively. Explicit
retry/discard reconciles the exact held row after storage becomes writable;
failed persistence leaves the review barrier in place.

This provides single-use queue admission, not exactly-once tool side effects.
It does not change ordinary direct-text `turn/steer` semantics, infer successful
execution from chat history, or automatically repeat an uncertain action.

## Verification

Regression tests cover Native RPC retry identity, original payload preservation,
concurrent edit/remove/transfer, turn-completion races, session isolation,
restart holds, fresh-identity retry, FIFO release and persistence failure.
Storage tests cover transaction rollback, multiple connections, ordering,
receipt retention and recovery. Desktop tests cover unsupported servers, lost
responses, remounts, stale turns, held controls and delayed admission replies.

```sh
cargo test -p devo-server --lib db::queue_steer::tests
cargo test -p devo-server --test atomic_queue_steer
cd apps/desktop && bun test --isolate
```

## Host-owned input

Host notification provenance remains attached to queue entries, including recovered entries. `session/queue/steer` rejects host-owned entries before a durable transfer or human message is created. Existing content-edit restrictions remain in place. Normal host follow-up delivery retains its source and non-authorization notice.

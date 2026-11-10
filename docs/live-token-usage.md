# Live context and session usage

Context occupancy and cumulative usage answer different questions:

- **Current context** is the latest model request in this session, divided by
  that model's effective context window. It is not session spend. Child requests
  have their own context windows and must not replace their parent's context.
- **Session usage** is cumulative provider-reported input, output, cache-read,
  cache-write and total tokens, including attributed child calls and overhead.
  Cache tokens are subsets of input, not extra tokens to add to the total.
  Use the authoritative `totalTokens` rather than reconstructing it from columns.

## Native projections

Clients consume these existing Native shapes:

| Source | Context | Cumulative spend |
| --- | --- | --- |
| `turn/usage/updated` | `usage.query.totalTokens`, `contextWindow` | `sessionTotals` |
| `session/usage/updated` | No context update | `usage.total` |
| Session read/resume/subscription snapshot | Separate occupancy read/resume anchor | `session.usage.total` |
| `context/usageUpdated` | `occupancy` | No spend update |

A completed model request updates the cumulative meter without waiting for the
agent turn or its tools to finish. Provisional provider samples may advance
context. They must not reset or replace cumulative counters with the latest
request's values. Replay/late lower cumulative snapshots are ignored. Session
switches reset client state before loading that session's own totals.

Zero or missing query placeholders preserve an already known context anchor.
Token-only updates retain the known effective window. An older in-flight context
read cannot overwrite a newer live notification.

`callCount` counts all attempts; `meteredCallCount` counts attempts with provider
usage. Missing usage is displayed as **unknown**, and partially metered sessions
are labelled **partial**. Historical totals without per-call detail do not invent
an input/output split. An absent `estimatedCost` is **unknown**, not a free call.
Known monetary amounts retain their currency and convert micro-units to units.

`unreportedFields` preserves per-field coverage across calls. A field omitted by
any request is unknown, even if other requests supplied a subtotal. A separately
reported total can still be known when its input/output split is missing. Explicit
reported zero remains zero. Old snapshots without coverage metadata are labelled
as having unknown coverage.

## TUI

`/context` and its `/usage` alias show current context separately from cumulative
input/output/cache/total and known estimated cost. The Native tree projection
exposes `cumulativeUsage`; unreported own/child breakdowns are `null`, not fabricated
zeros. The displayed session total already includes children and is not summed
again with child rows.

The activity loader's character-based count is explicitly marked `(est.)`. It is
not added to provider context or spend. Live context notifications update the
InteractiveMode state immediately, rather than waiting for an end-of-turn read.

## Desktop

The context ring remains latest-request occupancy. The session overview exposes
separate cumulative session usage, with input/output/cache columns, total tokens
and cost coverage. SDK cumulative usage events and renderer state are distinct
from context updates, and are restored from session snapshots.

## Verification

Adapter/store/render tests cover two model calls in one turn: latest request
230 tokens versus session total 350 (300 input and 50 output), cache tokens without
double counting, restore/replay, zero placeholders, late updates, child-session
isolation, unreported usage and unknown cost. These tests use the real adapters
and rendering code with fake transport; they do not claim a paid-provider or full
interactive TTY/Desktop end-to-end run.

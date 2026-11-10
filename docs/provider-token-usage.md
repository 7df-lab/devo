# Provider token usage normalization

Provider token usage is a reported measurement, not a local estimate. A response
without usage still completes a model call, but must not become a metered call
with an invented zero count.

## Presence and compatibility

`devo_protocol::Usage::default()` means no provider usage has been observed.
Consumers must call `Usage::is_reported()` before recording a usage sample.
Explicitly reported primary counters, including an input/output pair of zero,
set `reported: true`. A missing, null, empty, or metadata-only usage payload is
unknown. Empty streaming placeholders do not erase the last reported sample.

The additive `reported` field defaults to false on old serialized values.
Nonzero primary counters and present optional counters still count as reported
for compatibility with old persisted samples. A legacy all-zero sample with no
optional counters cannot be distinguished from the old default and remains
unknown. No count is inferred from text length or tokenization.

The primary integer fields retain their existing shape. In a partial provider
sample, an omitted primary field remains zero until that provider reports it;
the sample's presence does not imply all breakdowns were supplied.

## Provider behavior and timing

- OpenAI Chat Completions accepts optional usage fields and ignores empty usage
  chunks. A real usage chunk becomes `UsageDelta`; the final response retains
  the latest reported sample.
- OpenAI Responses exposes terminal usage in `MessageDone`. Reported top-level
  streaming usage is also emitted as `UsageDelta`, and is retained if the
  terminal response omits usage. A failed SSE response with usage emits that
  usage before the terminal error. The adapter's non-streaming API folds this
  same stream. The optional completion usage observer preserves those real
  samples if an auxiliary completion fails or is cancelled.
- Anthropic combines reported `message_start` and `message_delta` counters into
  cumulative snapshots. Cached input is included in the primary input count
  while cache creation/read counts remain available as separate breakdowns.
- Google currently calls `generateContent` and then synthesizes stream events
  from the complete response. Neither content nor usage is available before
  that HTTP response completes. Missing `usageMetadata` emits no usage delta.
  `promptTokenCount`, `candidatesTokenCount`, `thoughtsTokenCount`,
  `cachedContentTokenCount`, and `totalTokenCount` retain their provider meaning.

## Native ledger additions

`TokenUsage.totalTokens` is optional and preserves an explicit provider total.
Where present it is the authoritative display/aggregate total; otherwise totals
fall back to input plus output. In particular, Google total tokens can include
thought tokens outside its primary candidate count. When Google reports
thoughts without a provider total, the raw fallback is incomplete and
`totalTokens` is explicitly marked unreported. Reasoning and cache
breakdowns are never blindly added a second time.

`UsageRecord.parent` is optional captured attribution with `sessionId` and an
optional `turnId`. It lets a child call retain the parent owner that triggered
it, even if a different parent turn is active by the time the call completes.
Both additions default to absent when old ledger records are read.

The HTTP/SSE normalization matrix lives in
`crates/provider/tests/usage_presence.rs`. It uses actual provider adapters
against loopback test servers, rather than mocked normalized responses.

## Runtime accounting and replay

`InstrumentedProvider` assigns one stable `callId` to each attempt. Before a
completion or `MessageDone` is returned, it synchronously appends the terminal
`UsageRecord` to the existing canonical journal. A persistence failure fails the
request instead of silently claiming the usage was saved. Stream errors, EOF
without completion, and cancellation have separate outcomes. If an interrupted
stream reported a partial sample, that actual sample is retained; otherwise its
usage remains absent. No cost is invented when pricing is unavailable.

The same record projection drives live and restored session/turn totals. It
sums a call only once by `callId`, includes failed/cancelled attempts in
`callCount`, and increments `meteredCallCount` only for a reported sample.
`TurnQuery` contributes to query usage; turn-attributed auxiliary calls contribute
to overhead. Session-only auxiliary calls contribute to session totals and
`byPurpose`. `meteredCallCount` means a sample was reported, not a guarantee that
all provider billing categories or interrupted-call tokens were reported.

`session/usage/updated` carries `usage.total`. `turn/usage/updated.sessionTotals`
uses the same committed totals, never provisional streaming samples. Both
Native notifications and busy `session/read` / `session/list` reads update before
the agent turn completes. A single publication sequence rereads the ledger when
publishing, and actor merges reread it instead of installing an old turn's sum.
Final query usage is a must-deliver event; provisional samples remain best effort.

Child calls contribute to ancestor cumulative spend, while context samples and
`context/usageUpdated` stay scoped to the child session. Parent attribution is
captured when an attempt begins, so a late child cannot overwrite a later parent
turn or replace the parent's context. Nested child records retain their parent
turn lineage. A child request's own multi-call turn summary uses the same ledger.

On restart, records restore usage even if no completed turn was written. This
journal projection overrides and repairs stale SQLite totals. Journal stamps
invalidate cached projections after another process appends
records; a byte cursor reads only appended rows, retries a partial final row,
and restarts on replacement/truncation. Call IDs make replay idempotent. Only the requested list page is
enriched. Shared root journals are read by logical session identity; older
independent child journals are found through the session index. User forks are
not descendants for billing; copied historical turns retain
context but start with no spend. Old fork journals are identified by their
creation boundary.
Transcript rollback/supersession does not refund already-spent tokens.

For older journals, a turn usage summary written before the first call record is
an opaque legacy baseline, tracked separately for query and overhead. Later
summaries overlap the records and are not added again. Each query/overhead
component selects the last checkpoint preceding its first call record,
independent of the order in which older separate child journals are read. Baselines remain explicit `SessionUsage.legacy`; they are not fabricated
call records. Older child records without parent-turn attribution still
contribute to ancestor session totals; their exact historical parent-turn split
cannot be reconstructed. Restored context uses the session's own last known
query, and does not fabricate category percentages if only its total survived.
A later durable context/compaction checkpoint takes precedence over older query
pressure; a call admitted after that checkpoint supersedes it even if its token
counts happen to match the earlier query. Missing input fields do not erase the
last known input count.

Coverage is in `crates/server/src/usage_ledger/tests.rs`, the event-queue regression
test, and `crates/server/tests/live_usage_accounting.rs`. The runtime test blocks
a second model call after a real tool, verifies busy reads and notifications,
and copies the first-call journal into a fresh database before any turn-end
record exists. It makes no live paid-provider or complete GUI claim.

## Field-level coverage

`Usage`, `TokenUsage`, `UsageTotals` and `PurposeUsage` carry optional
`unreportedFields`. Its Native values are `inputTokens`, `outputTokens`,
`reasoningTokens`, `cacheReadInputTokens`, `cacheCreationInputTokens` and
`totalTokens`. A numeric zero is known only when the provider reported that
field. A provider total can be known independently of missing input/output
breakdowns. Without a provider total, both primary counters must be present to
know the derived total. Optional cache/reasoning counters remain unknown when
absent, even when primary counts are complete.

Aggregates take the union of missing fields across calls. Raw numeric values
remain the sum of known reports; clients show affected fields as **Unknown**,
not as complete totals or fabricated zero. A newly empty session explicitly has
an empty missing-field list. Old records without coverage metadata are treated
as unknown coverage, including their old zero-valued fields. An explicit
legacy turn total is preserved as known while its unproven breakdown is unknown.

An ancestor legacy baseline was already inclusive of historical child spend.
Replay does not add covered descendant legacy lumps to it again. Identifiable
old child records are split out of that opaque baseline before adding their
known calls. With only old inclusive summaries, the overlap of a child that
finished late cannot be recovered exactly; the recorded ancestor baseline is
preserved as unknown legacy rather than inventing a delta. New calls have
explicit attribution and do not have this ambiguity.

Final query/context notifications use the existing reliable outbound admission
policy, while provisional samples remain best effort. Reliable transport queues
still have the server's existing two-second timeout for a stalled client; this
change guarantees durable accounting, not lossless delivery over a disconnected
or indefinitely stalled transport. `session/read`, `session/list`, `turn/read` and replay
reconcile those clients with the ledger.

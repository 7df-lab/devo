# Model-call diagnostics

Journal behavior verified against main `a6ceaf8`; span correlation added here.

The Native runtime already records every finished model-call attempt in its
canonical session journal as an internal `usageRecord`. Use its `callId` as the
request identity, not a second trace journal:

- `sessionId` and optional `turnId` identify the caller.
- `purpose` distinguishes turn queries, compaction, title generation and other
  auxiliary calls. Retries are separate calls with separate IDs.
- `model` records the provider and wire model actually used.
- `parent`, when present, captures the triggering parent session/turn for a child
  call. It does not move to a newer parent turn when the child finishes late.
- `outcome` and optional `usage` distinguish success, failure and cancellation,
  including calls where the provider does not report token counts.

Provider diagnostics now run inside a `model_call` tracing span carrying that
same call ID, session, optional turn, purpose, provider and model. With debug
logging enabled, start/finish messages and logs from provider futures and stream
polls can be joined to the journal record by `call_id` / `callId`. Interleaved
calls keep separate spans; no entered span is held across an async yield.

Compaction diagnostics retain model/count metadata but no longer copy prompt or
summary bodies. These spans do not add prompts, response bodies, headers,
credentials, or provider configuration. This is not a global redaction guarantee
for all existing logs: review diagnostics before sharing them.

Compaction snapshots already link the summary item and preserved item IDs to
their session/turn. Child sessions already retain their parent relationship in
that same journal. This change adds no new storage, RPC, or per-item model-call
attribution. Calls made outside the Native runtime's metered provider wrapper do
not gain these spans. A process killed before an attempt can finish may have a
start log without a terminal journal record; the log is not a recovery contract.

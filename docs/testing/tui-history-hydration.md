# TUI history/live hydration

Initial transcript rendering and live events share the InteractiveMode event queue. The initial
barrier is installed before subscription (including synchronous notification callbacks), and holds
through history fetch and asynchronous tool renderer preload. A changed connection generation
invalidates stale work. Reconnect and tree navigation retain their existing queued replacement path.

Native snapshot metadata identifies completed items by stable item ID and revision. Buffered
notifications already covered by the snapshot are ignored; equal text with distinct IDs is not
collapsed. Paginated history folds duplicate IDs by revision and orders valid Native sequences.
Running assistant/reasoning items restore into the streaming row, rather than a second finalized
row. On completion, the canonical item text takes precedence over an observed delta suffix, which
may start midway through a response when attaching to a running agent.

## Verification

- Unit regression: `node --import tsx --test src/native-history-hydration.test.ts lib/coding-agent/src/modes/interactive/interactive-mode-hydration.test.ts` from `apps/tui`.
- Full TUI suite: `npm test` from `apps/tui`.
- Exercise initial open, nested child open, switching away/re-entering, and reconnect while the
  child is emitting text/tool updates. History must precede live content, with each durable item
  represented once. Exercise a delayed snapshot which includes an event already delivered live.
- Type-check output must be compared with the same main baseline if unrelated vendored errors
  prevent an aggregate pass. Do not describe this as a clean type-check.

The controlled transport PTY check verifies the actual InteractiveMode renderer and Native
adapter, not a real provider invocation or the Rust executable launch path. Provider smoke and
multi-client integration remain separate checks.

Reproducible controlled-transport terminal checks (from `apps/tui` inside tmux/psmux or a real PTY):

```sh
node --import tsx scripts/history-hydration-smoke.ts
node --import tsx scripts/history-hydration-smoke.ts --stream
```

The first fixture injects a live item before a delayed snapshot that includes the same item.
The second attaches to an in-progress assistant and delivers its complete canonical text later.
Both assert history-first ordering and one live row; neither creates a real model session.

### In-flight text boundaries

Native text deltas currently apply to the birth item (`baseRevision = 1` in
`crates/protocol/src/event.rs`); persisted running item text is not an accumulation of those deltas.
The adapter distinguishes a known birth/full base from a suffix-only mid-turn attachment.
Hydration seeds that base once. Already queued projected messages retain their own item identities,
so both thinking and text prefixes can be repaired before rendering, including a queued completion.
Canonical completion text wins over partial local accumulation.

Closed-stream tombstones survive turn cleanup in transport-local weakly associated metadata.
They stop a delayed running-item snapshot from reopening reasoning already closed with its assistant
message. Replacing the session stream state releases this metadata. No provider-specific text matching
or content-based duplicate detection is used.

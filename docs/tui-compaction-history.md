# TUI compaction transcript

Native `contextCompaction` items are projected into durable summary cards when
history is loaded. Successful cards retain their position in the active branch,
including when multiple compactions share a timestamp. The legacy pi
summary-first ordering rules do not apply to Native journal items.

Each card has its own padded block, a completed label and measured context token
counts. Unmeasured occupancy is shown as unknown, not as a measured zero.
Collapsed cards show a short preview; the standard Ctrl+O display cycle expands
the full summary. Both the main transcript and shared subagent conversation
renderer use this component.

Live completion rebuilds from the same history projection used by session
switch/resume. Failed or unfinished item envelopes are not successful summary
cards. Compatibility with the server's completed-lifecycle `Compaction failed:`
diagnostic is retained; ordinary successful summaries mentioning "skip" are not
mistaken for skipped operations.

Regression coverage lives in `apps/tui/src/native-compaction-transcript.test.ts`:
multiple compactions, Native and legacy ordering, occupancy metadata, paginated
reload, switched/recreated connections, live completion refresh, and rendering.

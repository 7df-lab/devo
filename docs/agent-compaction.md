# Agent-requested compaction at turn boundaries

`await compact.run(instructions)` schedules work; it does not compact context
inside the Python cell. The post-turn scheduler attempts the request after the
working set is merged back into the session actor.

A queued follow-up or another admitted turn can temporarily block compaction.
Keep the request pending in that case and retry at the next idle turn boundary,
after the queued work drains. A successful request produces the normal
`context/compactionStarted` and `context/compactionCompleted` notifications and
a persisted compaction snapshot. A scheduled response alone is not evidence
that compaction completed.

Requests remain last-write-wins: completion of an older attempt cannot discard
newer instructions. Interrupt still clears pending compaction; retry never
recreates a request that was canceled.

The integration regression queues a follow-up while the parent is completing
its `compact.run` turn, using semaphore gates rather than timing-dependent
sleeps. It requires a completed compaction notification after the follow-up.

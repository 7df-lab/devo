# Desktop automation lifecycle and background-runtime roadmap

## What this implementation guarantees

Desktop automation currently runs inside the Electron main process. Keeping a
window hidden or closed is not the same as quitting the application. This change
hardens that in-process scheduler; it does **not** introduce a daemon, an OS
service, or a 24/7 execution guarantee.

### Calendar and clock behavior

- The saved IANA `schedule.timezone` controls the schedule's wall-clock hours and
  weekdays, independently of the machine's current timezone. Preview and execution
  share the same date calculation. The creation dialog previews in the local zone
  it will save. Invalid zones and invalid rules produce no scheduled execution.
- Daily 09:00 remains 09:00 across DST. A nonexistent spring-forward local time is
  skipped. A repeated fall-back local time fires once at the earlier instant.
- An embedded RRULE `TZID` must match the saved timezone. UTC `DTSTART` is converted
  to that zone; a floating/TZID `DTSTART` supplies wall-clock fields. `UNTIL` is an
  absolute cutoff. The supported input is one RRULE, not a calendar/RRULE set.
- A live timer retains its recurrence anchor across executions. Rules without an
  explicit `DTSTART` are re-anchored on registration/restart; durable interval
  phase across restarts remains future work. Interval previews are estimates
  based on preview time rather than the live task's anchor.
- Timers use at most 60-second checkpoints. Long delays cannot overflow Node's
  signed 32-bit timeout limit. Each checkpoint rechecks the actual clock before
  execution, so a backward clock adjustment cannot trigger early work.
- If the process survives sleep or a forward clock jump, one overdue pending
  occurrence may run on wake. The next occurrence is then calculated after the
  callback completes. Missed occurrences are not replayed as a backlog.
- A replacement schedule cannot overlap the same automation's scheduled callback
  already in flight. This is an in-process scheduled-run guard, not a durable
  exactly-once guarantee or a manual-Run-now deduplication policy.

### Stop and restart behavior

- macOS: closing the last window leaves Electron running. Explicit Quit stops it.
- Windows: the normal main-window close action hides to the tray. Explicit Quit
  stops the process, local server, and automation scheduling.
- Linux: closing the last window follows the existing application quit behavior.
- Explicit quit never silently becomes “keep running in the background.” The
  scheduler stops and admits no new automatic work. In-flight actions may already
  have changed files or remote systems; stopping the process cannot undo them.
- On startup, before API calls or new executions are admitted, old `queued` and
  `running` rows become `failed` with an explicit **outcome unknown** interruption
  message. Their session ID, worktree, and result evidence are preserved. Failure
  counters and records are updated in one SQLite transaction, idempotently.
- Interrupted runs are **not automatically replayed**. Inspect the session and
  workspace before choosing Run now. Automations active in config are scheduled
  for their next future occurrence; downtime is not backfilled.
- Remove, pause, replacement, and stop invalidate pending date calculations. An
  asynchronous calculation cannot resurrect a cancelled timer. A cancelled or
  replaced in-flight callback cannot re-arm its former schedule on completion.

Startup recovery assumes the existing single Desktop owner of this database.
Multiple independent processes sharing the database, manual/scheduled execution
coordination, durable claims, and crash-safe side-effect deduplication are not
provided by this slice.

## Verification

Automated tests cover configured zones, fractional offsets, date rollover, both
DST transitions, half-hour DST, UTC cutoffs, interval phase within one process,
invalid input, long timers, clock rollback, sleep catch-up, asynchronous
cancellation, replacement during execution, callback failure, shutdown, and
transactional restart recovery against a real temporary SQLite database.

Run the calendar tests in separate processes with multiple host zones, for example:

```sh
cd apps/desktop
TZ=UTC bun test src/shared/automation-schedule.test.ts
TZ=America/Los_Angeles bun test src/shared/automation-schedule.test.ts
bun test --isolate
bunx tsc --noEmit
```

OS-level sleep/wake, tray, logout, reboot, and forced-process-termination tests
still require packaged applications on supported Windows/macOS/Linux hosts.
Unit tests do not establish those operating-system guarantees.

## Roadmap to a persistent background runtime

1. **Define ownership and the service contract.** Move execution ownership to one
   user-scoped runtime using the Native protocol. Desktop becomes an attachable
   client. Define process identity, authenticated local transport, version
   negotiation, status/health, cancellation, and explicit stop versus detach.
   Acceptance: detach/reattach does not restart a turn or lose its event cursor;
   incompatible clients fail visibly without spawning a competing owner.
2. **Persist schedule claims and run state.** Give every occurrence a durable
   identity, timezone, anchor, claim lease and fencing token. Persist intent before
   execution, transitions during work, and results before acknowledging completion.
   Define per-automation skip/coalesce/catch-up policy and concurrency limits.
   Acceptance: crash at each transition never creates two owners of one run;
   expired leases recover without blindly repeating an uncertain external action.
3. **Supervise the process explicitly.** Provide opt-in per-user launchd, Windows
   service/task, and systemd integrations with bounded restart backoff, log
   rotation, health checks, and upgrade handover. No silent service installation or
   expansion of unattended permissions. Acceptance: sign-in/reboot/upgrade tests,
   explicit Stop remains stopped, restart storms are bounded, and secrets remain
   within the existing credential boundary.
4. **Recover sessions and observe outcomes.** Reattach to Native sessions with
   durable event cursors, preserve artifact/worktree references, show interrupted,
   blocked, stale and unknown outcomes, and expose queue/lease/scheduler health.
   Approval requests must remain blocked until an authorized decision arrives.
   Acceptance: a disconnected Desktop can reconnect, find every child session and
   artifact, and distinguish a finished action from one requiring reconciliation.
5. **Prove long-duration operation.** Run multi-day soak and fault-injection tests:
   suspend, wall-clock jumps/DST, network loss, provider throttling, process kill,
   disk full, corrupt configuration, lease contention and upgrades. Publish
   measured recovery latency, missed/duplicate occurrence counts, resource usage,
   and supported host conditions. Only then describe the supported always-on
   behavior, with power/network/permission limits stated explicitly.

The daemon work must align with the subagent supervisor and Factory lifecycle.
An idle kernel cell is not an authorized background host lifetime: do not create
unowned async tasks merely to simulate durable execution.

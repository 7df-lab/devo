# Desktop subagent roster

The session header's **Subagents** button opens an automatically updating roster of its
saved direct children. Select a child to open its existing conversation. The
existing parent breadcrumb returns to the parent, and a child's own roster can
be used to inspect the next level. User forks and independent roots remain
normal top-level conversations; transient `/side` agents are excluded.

## Why this slice exists

Desktop intentionally hides child sessions from its main sidebar. It previously
had no child roster, and an unknown session ID was resolved by scanning the
default, root-only `session/list`. That lookup could not find a child after a
restart or missed creation event. Unknown IDs now use canonical `session/read`,
without resuming an actor or loading its history just to resolve the identity.

The roster uses the existing Native `session/list` contract with
`parentSessionId` set to the current session. It retains canonical status and
flags instead of projecting a second subagent lifecycle. Approval and input flags take precedence
over working; queued idle work is labeled **Queued**. **Idle** means no active
turn, not successful task completion or a promise that saved work will resume.
Opening the child lets the existing conversation/recovery UI show more detail.

## Live updates, reconciliation and cost

- While open, the roster applies validated Native creation, metadata, status and
  flag, archive, cwd, deletion and queue notifications immediately. The observer
  fans out existing transport traffic; it does not consume the main UI's
  single-reader event queue or create subscriptions that change approval routing.
- Existing directory subscriptions do not deliver every child queue event, and a
  child's cwd may differ from its parent's. A read-only reconciliation starts
  **30 seconds after the previous scan finishes**, only while the panel is open
  and connected. Window focus and runtime reconnect also reconcile automatically.
  There is no closed-panel polling and no per-event global roster scan.
- Open, reconciliation, Refresh and Check more never spawn agents, send messages,
  replay tools, resume work, or hydrate child transcripts. Refresh remains an
  optional retry, not a requirement for normal updates.
- Each scan requests at most five pages of 200 sessions, retaining only direct
  children. If a cursor remains, the UI exposes **Check more sessions** rather
  than presenting a complete empty result. Duplicate child IDs are deduplicated.
- Native `session/list` filters direct subagents by `parentSessionId` **before**
  pagination, so unrelated sessions cannot fill the five-page scan. The parent
  filter implies child inclusion even if `includeChildren` is omitted or false.
  It composes with cwd/title filters; Desktop leaves `cwds` empty so children
  whose cwd changed are not omitted. Unknown parents return an empty page.
  Equal activity timestamps are ordered by session ID to keep repeated pages stable.
- Desktop expects the matching Native contract shipped with it. If a runtime
  ignores the filter and returns an unrelated/ephemeral session, Desktop reports
  the mismatch and asks for a runtime update instead of silently scanning the
  global roster. There is no older-runtime discovery fallback.
  The five-page bound limits returned rows and round trips, not the server's
  underlying index/live-handle enumeration.
  A bounded scan is not proof that a previously loaded child disappeared: those
  rows are retained if the scan is incomplete. Live deletion events remove them;
  deletions missed while disconnected can remain until a complete scan/reopen.
  Every reconciliation restarts the pagination cursor at the beginning.
- Only one scan runs at a time. Focus/reconnect during a scan coalesce into one
  follow-up scan. Native events received after a scan begins overlay its pages,
  including deletion tombstones, so a late response cannot undo newer changes.
- Disconnect stops the timer and invalidates pending reads, retaining the last
  state with an explicit error. Reconnection reattaches and reconciles. Closing,
  navigating, switching servers, or unmounting removes listeners and timers and
  invalidates pending results. Requests already sent may still finish.
- Native cursor pages are not a transactional snapshot. Concurrent inserts or
  deletion can shift pagination; no completeness guarantee across concurrent
  mutations is claimed. The 30-second reconciliation is a bounded fallback, not
  a promise that all unobserved child changes are delivered instantly.

## Relation to the Python controls

PR #272 adds Python `rlm.collect`, progress notes, graceful end-turn, and independent
root creation. Those are not new public Native UI RPCs. This Desktop change works
on the existing Native surface and does not stack or duplicate #272's runtime
changes. It does not expose collection results, retained final answers, child
progress notes, or bulk steering/cancellation. Factory children use this same roster. Factory graph state and durable checkpoints
remain owned by the Python scheduler and host bridge; this is child discovery,
not a Native Factory start/stop or graph-control API. Those require a separate design
that preserves the runtime's recovery and admission semantics.

## Verification

Native integration tests cover parent filtering before pagination across more
than 1,000 unrelated rows, deterministic tied timestamps, cold index rows with
unreadable journals, cross-cwd children, composed title/cwd filters, unchanged
root/fork discovery, and live child status. Desktop tests cover unscoped-response
errors without a global-scan fallback, exact-ID child discovery, Native validation, read-only requests,
multi-page filtering, bounded scans, duplicate IDs, repeated cursor errors,
close/reopen and navigation races, repeated refreshes, error/retry behavior, and
real Base UI popover opening, Escape/Close dismissal, focus restoration and child
navigation. Live regression cases cover validated passive fan-out without stealing
main UI events, status/flag/queue and creation/deletion updates, events racing old
pages, one scan at a time, the 30-second fallback, disconnect/reconnect and focus,
server switching, and listener/timer cleanup. The roster does not change the existing visual tokens or icon scale.

Packaged Electron, visual screenshot and real-provider end-to-end checks remain
separate from these component and transport tests.

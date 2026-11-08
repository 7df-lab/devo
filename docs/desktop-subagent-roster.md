# Desktop subagent roster

The session header's **Subagents** button opens an on-demand snapshot of its
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
`includeChildren: true`. It retains canonical status and flags instead of
projecting a second subagent lifecycle. Approval and input flags take precedence
over working; queued idle work is labeled **Queued**. **Idle** means no active
turn, not successful task completion or a promise that saved work will resume.
Opening the child lets the existing conversation/recovery UI show more detail.

## Refresh and cost

- Open, Refresh, and Check more are read-only. They do not spawn agents, send
  messages, replay tools, or resume work. There is no closed-panel polling.
- Each scan requests at most five pages of 200 sessions, retaining only direct
  children. If a cursor remains, the UI exposes **Check more sessions** rather
  than presenting a complete empty result. Duplicate child IDs are deduplicated.
- The Native list API has no parent filter. Pages enumerate the global roster so
  children whose cwd changed are not omitted. The five-page bound limits returned
  rows and round trips, not the server's underlying index/live-handle enumeration.
  Parent-filtered listing or a dedicated subscription would be needed for an
  efficient continuously live view at large scale.
- Refresh failures preserve the previous snapshot with an explicit error.
  Closing or switching the session invalidates pending results and stops further
  page requests. A request already sent to the transport may still finish.
- Native cursor pages are not a transactional snapshot. Concurrent inserts or
  deletion can shift pagination; Refresh starts a new scan. No completeness
  guarantee across concurrent mutations is claimed.

## Relation to the Python controls

PR #272 adds Python `rlm.collect`, progress notes, graceful end-turn, and independent
root creation. Those are not new public Native UI RPCs. This Desktop change works
on the existing Native surface and does not stack or duplicate #272's runtime
changes. It does not expose collection results, retained final answers, child
progress notes, or bulk steering/cancellation. Those require a separate design
that preserves the runtime's recovery and admission semantics.

## Verification

Tests cover exact-ID child discovery, Native validation, read-only requests,
multi-page filtering, bounded scans, duplicate IDs, repeated cursor errors,
close/reopen and navigation races, repeated refreshes, error/retry behavior, and
real Base UI popover opening, Escape/Close dismissal, focus restoration and child
navigation. The roster does not change the existing visual tokens or icon scale.

Packaged Electron, visual screenshot and real-provider end-to-end checks remain
separate from these component and transport tests.

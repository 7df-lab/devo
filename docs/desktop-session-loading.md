# Desktop session loading

## Loading and subscriptions

Desktop loads folder lists when their sidebar sections are expanded. Discovery
must finish before a section starts loading; requests made before a connection
exists must not leave the section permanently marked as loading.

Each SDK client subscribes to a lightweight `sessionsByCwd` roster per directory.
Idle sessions do not need individual transcript subscriptions. Opened sessions
and sessions that become active receive a session subscription. Folder snapshots
use SQLite metadata and the active-turn registry, without parsing every rollout
or waiting on session actors. Canonical model and settings obtained from resume
are preserved when a lightweight roster refresh arrives.

`subscription/create` accepts optional `replay: "snapshotOnly"` together with
`includeSnapshot: true`. This registers the live subscription at its barrier and
returns current snapshots, pending controls, and in-flight text recovery without
replaying historical events. Omitting `replay` retains existing cursor replay;
ACP behavior is unchanged. Recovery reads occur after releasing the connection
barrier, so a running turn can still publish events.

## Transcript pagination

Native history pages accept opaque cursors:

- No cursor, or an existing numeric cursor: ascending history, unchanged.
- `tail`: the newest page, ordered chronologically.
- `before:<sequence>`: the preceding page, also ordered chronologically.

`nextCursor` continues in the requested direction. Page limits are clamped to
1–200. The desktop initially requests 160 recent items, then requests only the
additional history needed when scrolling upward. A caller omitting a history
limit can still load the complete transcript. Concurrent reads share requests
and recheck coverage after a narrower pending read finishes.

Queue readiness resumes/subscribes the session without downloading its history.
Resume releases the session state-change gate before queue recovery acquires
that gate again. Reconnect invalidates pagination/readiness state; superseded turns invalidate
pagination state.
Rapid navigation prevents an older chat's completed fetch from changing the
current chat's loading indicators.

## Windows QA evidence

The October 9, 2026 QA run used a private Electron profile and Native home,
120 fixture sessions in one folder, and a 2,400-item transcript. Protocol tracing
showed the transcript's initial read drop from twelve 200-item requests to one
160-item request; scrolling fetched preceding 100-item pages. Queue reads did
not fetch history. Individual warm reload observations were approximately 5.4s
before and 2.5s after; these are observations from one machine, not benchmarks.

Actual Electron interactions covered real Codex Python execution, queued input,
live steering, switching chats during work, model catalog refresh, persisted
provider availability, theme/notification controls, and Settings navigation.
The queued and steered instructions remained visible after completion and the
model responded to both. Console capture exposed and led to fixes for shared
state updates during layout rendering and Git worktree reads for plain folders.
The Electron development CSP warning is expected in the Vite development build.

Regression coverage includes tail pagination, bounded SDK reads, request
coalescing, lazy subscriptions, transient text recovery, startup connection
races, plain-folder worktree lists, and concurrent Native reads/queue/steer/
interrupt/resume while a provider is working.

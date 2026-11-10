# Independent Native runtime (Unix)

This adds an explicit process owner for Devo's existing Native runtime. It reuses
the same session actors, singleton lock, authenticated proxy, durable journals,
interval schedule store and schedule outbox. It does not create a second agent
engine or install an operating-system service.

## Commands and ownership

- `devo daemon start`: launch a detached child, without inherited stdio or a
  controlling terminal. Report ready only after an authenticated status reply.
- `devo daemon run`: foreground owner for an operator-managed supervisor.
- `devo daemon status`: authenticated live status; no owner is created.
- `devo daemon attach`: NDJSON Native connection to an existing daemon. Missing
  owner is an error, never an implicit server launch.
- `devo daemon stop`: authenticated graceful stop; wait for the singleton lock to
  release before reporting stopped. A 30-second drain timeout is an uncertain
  outcome, not permission to kill a PID or start a replacement.

Start the daemon before opening Desktop/TUI. Existing `devo server --transport
stdio` clients attach through their proxy processes. Closing those clients does
not end the daemon. A client-owned server is never silently promoted, killed or
replaced: explicitly stop it first. Legacy `devo server --shutdown`, including
Desktop protocol-recovery attempts, refuses to stop a daemon. The server itself
rejects the old shutdown wire method for a daemon, including from older clients.
Use `daemon stop`, which sends the explicit daemon lifecycle control.

A start readiness timeout means startup may still be in progress. Check status
before retrying. There is no automatic restart, login startup, upgrade handover,
or service installation in this change. OS power, suspend, network access,
provider availability and approvals still constrain execution; this is not a
24/7 availability promise. Windows daemon commands fail closed until owner-only
ACL handling is implemented and tested; existing client-owned Windows behavior
is retained.

## Security and shutdown

The daemon uses only the random internal loopback listener. It does not expand an
empty listener list, honor configured public WebSocket listeners, or read stdin.
The proxy requires a random token before Native/control traffic. Metadata is
published atomically with owner-only Unix permissions. Lock/metadata paths reject
symlink following, non-regular files and wrong owners. Startup checks directory
ancestors for cross-user replacement risk. Discovery accepts only numeric loopback
`ws` endpoints, with no credentials, query, fragment or external hostname.

The proxy limits concurrent connections to 32, with at most 16 long-lived clients
so controls retain capacity, message/frame size to 16 MiB and
handshake, token and initial request waiting to five seconds. Stop closes admission
and drains existing requests before snapshotting runtime shutdown. SIGTERM and
SIGINT follow the same graceful owner teardown. A blocked drain remains visible as
a stop timeout rather than claiming completion. No permission preset, approval
answer, unattended tool privilege or credential grant is changed.

**Same-user local processes and agent tools are not isolated by this token.**
The existing sandbox permits access to DEVO_HOME, and same-user code can read the
control token or edit runtime files. This architecture is not a security boundary
against hostile local tools or prompt-injected code. Enforcing that boundary
requires a separate protected control-credential store plus sandbox/IPC capability
work. Do not describe owner-only file permissions as agent-tool isolation.

## Persistent Native schedule recovery

Native interval schedules and their outbox survive owner restarts. Startup must
finish rebuilding the session index from durable journals before the wake loop can
decide that a schedule's session is missing. This prevents an empty/rebuilt SQLite
index from deleting valid scheduled work. Missing-index recovery also checks the
durable journals strictly: present, unreadable or damaged journals preserve the
job/outbox for retry instead of interpreting an index failure as deletion.

The next occurrence keeps its persisted interval phase after delayed wake or
restart. Missed slots coalesce into one delivery rather than a catch-up storm. At
most one undelivered occurrence per job is added to the outbox, so offline,
busy or approval-blocked sessions cannot accumulate an unbounded interval backlog.
The outbox is written before advancing the job; recovery reuses the same occurrence
identity if a crash separates those writes. Pending queue/journal evidence is
reconciled before redelivery.

These are durable admission and deduplication properties, not exactly-once
external side effects. Interrupted or unknown tool outcomes still require review.
A model/tool call can have reached a remote system before any local completion was
persisted. Approval and question waits must never be automatically approved to
make background execution appear healthy.

Desktop RRULE automations remain separate Electron-owned jobs. PR #269 hardens
that layer; this Native daemon slice does not silently migrate or double-run those
jobs. Converging RRULE/timezone configuration and Desktop run history onto Native
scheduling is a later explicit migration with one authoritative owner.

## Relation to Prime and remaining work

Prime separates resident workers from attaching clients, tracks ownership and
session leases, and persists recovery/scheduling evidence. This change adopts the
independent-owner and explicit-attach lifecycle using Devo's existing singleton
instead of copying Prime's protocol or worker engine.

The comparison used Prime snapshot `74591cddef28d11249c3171112114fc4755375d7`:
`crates/pa-daemon/src/ownership.rs` separates client ownership and promotion;
`lease.rs` fences canonical session files with owner identity and process-start
evidence; `scheduled_jobs.rs` connects persistent cron/heartbeat storage to queue
hooks and recovery journals. Devo's existing singleton already supplies one owner
per DEVO_HOME, so this change makes that owner's lifetime explicit instead of
adding Prime's per-worker lease/supervisor model. Automatic worker retry, promotion
and multi-process per-session hosting are not implemented by these commands.

Still required for complete background parity: Windows ACL/service support,
Desktop schedule migration, explicit detach UI and daemon health, reconnectable
approval controllers, protected capabilities against untrusted same-user tools,
operator-approved restart policy, version/upgrade handover, resource quotas/log
retention, and multi-day OS suspend/reboot/fault-injection validation.

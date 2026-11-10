# Desktop development

From `apps/desktop`, run `bun run dev`. This regenerates Native protocol
types, builds the Rust runtime, and starts Electron with the renderer dev
server. `bun run preview` uses the same runtime preparation for a preview
of the production renderer.

## Windows executable locks

Windows prevents Cargo from replacing an executable while it is running.
Desktop development therefore builds the CLI and Windows sandbox helpers
in `target/desktop-dev`, rather than `target/debug`. If `CARGO_TARGET_DIR`
is set, the desktop build uses a `desktop-dev` subdirectory there.
The first build in this directory needs to compile its own dependencies.

Each Electron launch receives a separate temporary copy of the CLI,
sandbox helpers, and runtime DLLs through `DEVO_DESKTOP_DEVO_BIN`.
Rebuilding with `bun run dev:cli` while the app is running leaves that
instance on its current runtime. Restart `bun run dev` to use the new build.
Existing CLI sessions using `target/debug/devo.exe` can remain open.

An explicit `DEVO_DESKTOP_DEVO_BIN` override still selects that runtime.
Temporary copies are removed when the launcher exits normally. An abnormal
termination or a surviving child process can leave a temporary
`devo-desktop-dev-*` directory until the process exits and it can be removed.
Packaged applications continue to use their bundled runtime.

## Removing a project folder

Removing a folder from Desktop interrupts its active sessions, discards their
queued and steering inputs, and permanently deletes their histories. The
workspace directory and its files remain on disk. Other project folders and
their sessions remain available.

The renderer deletes sessions with up to eight concurrent requests. The
runtime cancels a session's subagent tree together, allows a 500 ms turn
finalization window, then aborts unresponsive turns. Kernel shutdown also has
a 500 ms graceful window before forced termination. Background commands are
stopped across connections even when their session has no active model turn.
Confirmed deletions leave the sidebar immediately. Failed deletions retain
the folder and its connection so removal can be retried.

## Queued follow-up controls

Dragging queued messages tracks stable queue IDs and resolves Native's one-based
positions at drop time, so draining a previous message cannot move the wrong
entry. Text-only editing is unavailable for messages with non-text context so
editing cannot silently discard attachments, skills, or mentions.

Steering uses one Native `session/queue/steer` request with the session, queue
identity, and expected active turn. The server claims the stored input and
persists an admission receipt. The client never resubmits the input or removes
it in a second request. An accepted receipt triggers an authoritative queue refresh; it never by itself
removes a row that may have returned as held recovery input. `keptQueued` also refreshes
the queue when the turn cannot accept input (for example, a stale turn
or pending approval). Unsupported servers
show an error without falling back to the unsafe two-request operation.
Direct-text `turn/steer` is unchanged.

Pending operations and errors belong to their originating project and session;
navigation cannot redirect an in-flight action. Repeated clicks are locked while
a request is pending. Local pending/error state is not durable across unmounts
or restarts. Safety after a lost response or remount instead comes from the
server's durable admission receipt: repeating the same session and queue identity
retrieves the same outcome, even when the client has no cached input or turn.

Entries marked `recoveryRequired` are held rather than automatically released.
They cannot be steered or edited, and all queue reordering is disabled while any
held row exists so another entry cannot cross it. The row warns that prior effects
may have occurred and retrying may repeat them. An explicit **Retry** invokes
`session/queue/retry`, which creates a fresh queue identity; the SDK refreshes the
queue after success or failure but never retries recovery automatically. Discard
remains available. Durable admission deduplicates transfer, not arbitrary tool
side effects: explicit recovery can repeat effects from interrupted execution.

# `devo-kernel`

CPython RLM REPL host (protocol v3). Spawns `python -m rlm.repl`.

## What this crate is

`devo-kernel` does not run Python code itself. It starts a long-lived Python
process, keeps it alive, and talks to it over stdin/stdout. The wire format is
newline-delimited JSON: one complete JSON object per line, UTF-8, nothing else.

Two names are used throughout this crate:

- **host** — the Rust side, meaning this crate. It spawns and owns the child
  process.
- **kernel** — the Python side, the process started by `python -m rlm.repl`.
  This is not the operating-system kernel.

The kernel is a persistent REPL. The model's Python code runs in one
`__main__` namespace on one asyncio event loop, so variables, imports, and
background tasks survive from one turn to the next.

```
+-------------------------------+                      +---------------------------------+
| Rust host (`devo-kernel`)     |  newline-delimited   | Python kernel                   |
|                               |  JSON on stdin/stdout| `python -m rlm.repl`            |
| `KernelSession`               | <------------------> | persistent `__main__` namespace |
|  - spawns and owns the child  |                      | one asyncio event loop          |
|  - reads events from stdout   |                      | `bash()`, MCP, harness, skills  |
|  - answers `host_request`s    |                      |                                 |
+-------------------------------+                      +---------------------------------+
```

One execute call looks like this:

```
model tool call
  -> host writes {"type":"execute","id":"c1","code":"..."} to kernel stdin
  -> kernel runs the cell and streams events tagged with id "c1"
       stdout / stderr / result / display / host_request / error
  -> kernel writes {"event":"done","id":"c1","status":"ok"}
  -> host returns one `CellOutput` to the caller
```

The crate also exposes interrupt, snapshot, restore, `list_names`, and shutdown
over the same protocol, plus a bounded, name-only `namespace_brief` for
[post-compaction context](../../docs/python-kernel-compaction.md). Alongside
`execute`, these make up the whole surface
between the two processes.

The Python package itself is described in
[`rlm-runtime/README.md`](rlm-runtime/README.md).

## Sessions and kernel processes

A session owns one kernel process. The server starts that kernel when the
session's first RLM turn begins and reuses it for later turns, so its Python
namespace stays warm.

- Two concurrent RLM sessions run two Python child processes. Each has its own
  `__main__` namespace, imports, background asyncio tasks, and `bash()` child
  processes. They share no Python state.
- A subagent is a separate session, so each running subagent also gets its own
  kernel. A parent and its child never share one.
- Within a session, the trusted kernel is reused while the workspace root and
  collaboration mode stay the same. Changing either shuts the old kernel down
  and starts a new one, restoring the session namespace from its snapshot when
  one exists. Plan mode does not make native Python read-only.
- `Discrete` execution starts no kernel. `devo prompt` starts one temporary
  kernel and shuts it down after the query.
- Deleting a session terminates its kernel and discards its namespace; other
  sessions are unaffected.

The crate does not pool kernels. `KernelSession` is owned per session by the
session actor (`crates/server/src/runtime/session_actor/state.rs`), so process
count grows with the number of sessions that actively use Python.

## Glossary

| Term | Meaning |
|---|---|
| host | The Rust side of the pair; this crate. |
| kernel | The Python side; the `python -m rlm.repl` process. |
| session | One host instance, one kernel process, and one Python namespace. |
| cell | One `execute` request plus all events up to and including its `done`. |
| event | A message from kernel to host: `ready`, `stdout`, `stderr`, `result`, `display`, `host_request`, `error`, or `done`. |
| `host_request` | A call from Python to Rust when kernel code needs the host to act: bash approval and execution, web search/fetch, MCP token refresh, subagents, and so on. |
| pump | The single background task that reads kernel stdout and routes each event. |
| idle route | The narrow path for `host_request`s that arrive while no cell is running. All other idle requests are denied. |
| execution surface | The set of tool schemas advertised to the model. With a kernel it is exactly `ipython`. |
| `Discrete` | Execution mode that does not start a kernel, so no `ipython` tool is advertised. |

## Where to start reading

| Order | File | Why |
|---|---|---|
| 1 | `src/protocol.rs` | All request and event types on the wire. |
| 2 | `rlm-runtime/src/rlm/repl.md` | The behavior behind those types. |
| 3 | `rlm-runtime/src/rlm/repl.py` | Kernel implementation. |
| 4 | `src/session.rs` | Host implementation. |
| 5 | `src/session/session_event_pump.rs` | Event routing and the idle route. |

## Trust boundary

Product Python is trusted code running with the user's OS privileges in both
Build and Plan. The kernel launcher does not apply an OS sandbox.
Native Python can read or write files,
use the network, and create subprocesses with those privileges. Plan is not an
OS read-only guarantee for Python. Host-mediated actions still use their host
policy checks, including Plan guards and configured shell sandbox policy.

Provider credential environment stripping remains in the kernel launcher. This
reduces accidental inheritance; it does not isolate trusted Python from files
or other resources the user can access. Per-session temporary directories,
namespace snapshots, and shutdown behavior remain in place.

## Runtime path

Set `DEVO_RLM_RUNTIME_SRC` to the directory that contains the `rlm` package,
or use the default `crates/kernel/rlm-runtime/src`.

On Windows the REPL uses an IOCP event loop with a named-pipe wakeup channel.
The pipe keeps thread wakeups and asynchronous subprocess I/O working without
requiring a loopback TCP connection.

## Session deletion

Session deletion first attempts a short graceful shutdown, then calls
`KernelSession::terminate` to stop and reap an unresponsive kernel. Deleting a
session discards its Python namespace immediately; it does not wait for a
running cell to finish. Normal shutdown retains the longer graceful timeout.

## Host requests

`KernelSession` uses one session-owned stdout event pump. `HostRequestHandler`
is captured for the live request and matched by the REPL `cell_id`; it cannot be
reused for a late detached request or a later turn. Replies use a **separate
stdin lock** (Prime deadlock rule). `set_idle_bash_completion_handler` is the
narrow idle route for Bash completion/consumption notices and session-scoped
`bash.poll` / `bash.kill` / `bash.forget` controls. Starting a command still requires the live
host authorization path. All other idle requests are denied, as are requests
when the applicable handler is unset.

### Python Bash API

`bash(command, ...)` schedules `bash.start` through `host_request` and returns a
synchronous, awaitable `BashHandle`. It never falls back to a local subprocess
when the host is absent or denies authorization. It requires a running asyncio
loop. The initial state is pending while the host authorizes and starts the
command. The creating cell stays active until its scheduled starts are admitted;
a denied background start is a cell error, not a silent detached failure.
`await handle` returns `BashResult(exit_code, output, duration)`.

- `handle.handle_id` is an opaque session-scoped string, initially `None`.
  `handle.pid` is always `None`; a host ID must not be used as an OS PID.
- `running`, `poll()`, `output()`, and `tail(n=50)` read cached snapshots without
  blocking the event loop. An async monitor refreshes them every 0.1 seconds
  while the command runs. `poll()` returns `None` until completion, then the
  result. Host failures surface from awaiting or polling the handle; output()
  and tail() retain the captured bytes for inspection even after failure. Terminal
  host failures without an exit status raise `RuntimeError` with the host error
  and captured output; the facade never fabricates an exit code.
- `kill()` schedules host cancellation, including during pending admission.
  It takes no signal or grace arguments. Await the handle to observe exit.
- Cancelling a first await of an otherwise untouched handle schedules host
  cancellation. Accessing a handle first marks it as background; cancelling
  later awaits does not kill it. Pending approval cannot be dismissed by this
  API: cancellation is applied when admission returns. Cleanup after connection
  loss is the host's responsibility.
- `workdir` defaults to Python's current working directory at the call. Explicit
  relative paths are resolved by the host. Shell selection and environment are
  host-owned: Python `os.environ` changes are **not** propagated to commands.
- Permission keywords match `rlm.shell`: `sandbox_permissions`, `justification`,
  `additional_permissions`, `prefix_rule`, and `timeout_ms`. `rlm.shell` remains
  a one-shot host request; it does not return a live handle.

Snapshots contain `handle_id`, `running`, `exit_code`, `output`, and `duration`
(seconds). `bash.poll`, `bash.kill`, and `bash.forget` take only `handle_id`.
The monitor forgets completed host entries after caching their final snapshot;
results remain readable locally. Host retention limits backstop failed forget
requests. Completion notices do not depend on registry retention. No PID is fabricated.
The old standalone local implementation is explicitly available as
`from rlm.local_bash import bash`; it is not the product default or an automatic
fallback. It runs directly in the trusted Python process's environment and does
not provide host shell authorization.

Default pipe-mode Bash may complete after its IPython cell ends. `bash.completed`
with `{handle_id, command, exitCode}` queues a follow-up; `bash.consumed` with
`{handle_id, command}` withdraws it when a live cell consumes the result.
Notices wait for the creating cell's completion barrier, so foreground awaits
do not also produce background follow-ups. Legacy explicit local handles still
use PID-scoped notices. Notices and handles are process-local, not recovered
across server/kernel loss. The `application/vnd.bash-activity+json` display
marker carries `id`, `active`, and (once available) `handle_id`; it is not part
of a client tool result and has no TUI/Desktop consumer.

Interactive and PTY tasks instead use host-managed completion messages in the
owning session's FIFO inbox. A foreground `await h` / `await h.wait()` sends
`bash.consumed` with `{handle_id, command, managed: true}` and waits for the durable
acknowledgement before returning (or raising a terminal execution error). The host
suppresses unpublished or still-pending completion messages; already-claimed turns
are not retracted. Partial output reads and detached waiters do not consume delivery.
This durable notification receipt does not restore processes after a server restart.
See [shell tasks](../../docs/shell-tasks.md) for exact lifecycle and timeout boundaries.

Server dispatch lives in `crates/server/src/runtime/kernel_host.rs`.

## Execution surface

When a session kernel is available, the regular function-tool list contains
exactly one schema: `ipython` (the Python kernel). Rust registry assembly hides
shell, agent-control, MCP, and other local schemas; ToolSearch cannot reveal
them, and the model-query runtime rejects unadvertised tool calls. The handlers
remain available to trusted host paths. Provider-native hosted web search/fetch
remain separate capabilities and are sent only when configured for the active
provider. If the kernel is unavailable or execution is configured as Discrete,
the regular function-tool list is empty; configured provider-hosted web tools
may still be sent.

The one-shot `devo prompt` command starts a temporary kernel for `ipython` and
shuts it down after the query. Its host bridge supports configured local web
search/fetch; interactive plan display and user questions are unavailable in
this one-shot mode.

# Python shell tasks

The model uses Python `bash()` for both ordinary commands and interactive tasks.
All actual processes are owned by the Devo host, not by Python `subprocess`.
Approval, sandbox selection, working directories and process cleanup remain host
responsibilities. No additional top-level model tool is needed.

## Ordinary commands remain compatible

```python
result = await bash("git status")
print(result.exit_code, result.output)

h = bash("a-long-running-command")
print(h.running, h.tail(20))
result = await h
```

`bash()` is **not an async function**. It schedules admission and returns a handle
immediately; `await h` waits for **exit**, not startup. Do not write
`h = await bash(...)` when you intend to interact with the running process.

The default `mode="pipe"` retains the old behavior: stdin is closed, and an
otherwise untouched one-shot await owns cancellation. `h.kill()` schedules host
cancellation, including while start approval is pending. It cannot dismiss an
approval dialog. No approval failure falls back to local Python execution.

## Interactive pipes

Use `mode="interactive"` for programs that accept ongoing stdin without needing
terminal emulation. For example, a connection to a local test server:

```python
h = bash("nc 127.0.0.1 9000", mode="interactive")
await h.ready()                   # host has spawned the process
await h.send_line("hello")        # explicit newline
chunk = await h.read(cursor=0, wait_ms=1000)
print(chunk.text)
cursor = chunk.next_cursor

await h.write(b"another message\n")
chunk = await h.read(cursor=cursor, wait_ms=1000)
print(chunk.data)                 # original bytes, including non-UTF-8
cursor = chunk.next_cursor

await h.close_stdin()             # close pipe input; it may cause the peer to exit
result = await h.wait(timeout_ms=5000)
```

`nc` is an example external program, not a bundled dependency. EOF behavior also
depends on the program and platform. A wait timeout does not kill the process.
Use `await h.terminate()` to request cancellation and then `await h.wait()` if you
need confirmation that it exited.

## PTY mode

Use `mode="pty"` only for programs needing an actual terminal:

```python
h = bash("python -i", mode="pty")
await h.ready()
await h.resize(cols=120, rows=40)
await h.send_line("print(1 + 1)")
chunk = await h.read(wait_ms=1000)
print(chunk.text)
await h.terminate()
await h.wait()
```

PTYs may echo input and transform line endings; stdout/stderr are merged.
`close_stdin()` is rejected for a PTY: there is no portable reliable half-close.
Send the program's exit command, or explicitly terminate it. A control byte such
as Ctrl-D is terminal-mode-dependent and is not a guaranteed EOF operation.

The cursor describes combined reader arrival order. `chunk.chunks` preserves
individual `BashChunk(stream, data)` segments with `stdout`/`stderr` labels for
pipes and `pty` for terminals. It does not promise exact ordering of writes made
to separate OS pipes. Separate stream cursors are not part of this API.

## Handle operations

- `await h.ready()` waits for successful spawn or raises its error. It is not a
  check that the application inside the process has finished initializing.
- `await h.write(str_or_bytes)` writes at most 65,536 encoded bytes without adding
  a newline. `await h.send_line(text)` adds one newline. Failed writes are not
  automatically retried: a failure can occur after partial input reached the
  process, so blind retries may repeat an action.
- `await h.read(cursor=0, max_bytes=8192, wait_ms=0)` reads at most 65,536 bytes;
  the maximum wait is 30,000 ms. A read timeout returns no new data, not a process
  failure. The returned `BashRead` has `data`, `next_cursor`, `truncated`, `eof` and `chunks`.
- Reads do not consume shared bytes. Each caller carries its own `next_cursor`.
  Omitting the cursor starts at zero again, not after the previous read.
- `chunk.text` is a UTF-8 convenience view with replacement. For binary data or
  UTF-8 characters split across reads, use `chunk.data` and an incremental decoder.
- `h.tail(n=50)`, `h.output()`, `h.running` and `h.poll()` are cached snapshots;
  they do not perform synchronous RPC.
- `await h.wait(timeout_ms=...)` is wait-only. Its timeout does not cancel the
  process. `await h` remains supported.
- `await h.terminate()` waits for the cancellation request's acknowledgement,
  not necessarily process exit. `h.kill()` is the non-awaiting form.
- `await h.resize(cols=..., rows=...)` is PTY-only; dimensions must be 1–1000.

## Output bounds

`h.tail()` keeps the most recent 256 KiB rather than freezing after the first
256 KiB. The first retained line may be partial. Cursor reads explicitly set
`truncated` when the requested position has been evicted from the rolling buffer;
callers must not treat that as a complete transcript.

`h.output()` and `BashResult.output` keep their bounded formatted-output semantics,
including final cancellation/timeout diagnostics. They are not aliases for the
rolling tail. Full-output capture, when configured by the host, is separate from
these in-memory views. Interactive tasks do not grant unlimited memory or disk.

## Lifetime and reattachment

Interactive tasks belong to their creating agent/session. They can continue
across Python cells and model turns. Unlike ordinary one-shot commands, cancelling
an await on an interactive handle does not implicitly kill it.

While the host still owns the task, a new Python proxy can be recovered:

```python
tasks = await bash.list()                 # current agent/session only
for task in tasks:
    if task["mode"] in ("interactive", "pty") and task["running"]:
        h = await bash.attach(task["handle_id"])
        await h.ready()
        break
```

`attach` is for interactive/PTY tasks and cannot access a different agent's task
by guessing its ID. Completed records have a finite retention period; after
expiry, attachment fails. Attaching a completed task does not spawn another
process. A retained cancelled, timed-out or failed task can still be attached
and read for diagnostic bytes; `wait()` continues to raise its execution error
and input writes remain rejected.

TUI disconnects, compaction and Python kernel replacement must not be confused
with explicit session deletion. The host owns the interactive process; frontend
connections and Python handles are proxies. Explicit task/session teardown still
cleans up processes. This does **not** promise recovery across a Devo server
restart or machine reboot. An old handle may no longer exist after restart; do
not automatically rerun its original command, which could repeat side effects.

The `timeout_ms` start argument is an optional total runtime limit. Interactive
and PTY tasks additionally accept `idle_timeout_ms`, a positive millisecond
limit reset by real input or output activity; both limits default to absent.
Polling a handle does not count as activity. There is no implicit “kill after
30 minutes without stdin” policy for this API. A per-operation read timeout is
independent of these task limits.

## Permissions and notifications

An interactive process can execute additional instructions after launch. Its
approval must therefore describe ongoing interaction, not falsely imply approval
of only a single static command. Shell access does not override the session's
sandbox, network, workspace or permission settings. Do not send credentials in
examples, log them by default, or assume a process's echoed input is safe to share.

The host routes completion to the owning agent through the existing message
queue. Python handles must not generate duplicate completion notices for these
host-owned interactive tasks. Output reads are explicit; every log line is not
injected as a new agent message. A generic process cannot reliably be classified
as “waiting for input” without application-specific protocol knowledge.

### Completion notification boundary

The host emits one managed terminal notification through the owning agent's existing
inbox. Interactive Python proxies do not emit a second completion event, including
when multiple proxies attach to the same handle. Reading or awaiting the retained
result does not currently withdraw an already published managed notification:
a synchronous waiter may therefore also see that notification later. Coordinating
synchronous consumption with durable inbox delivery is a separate follow-up; this
API does not promise exactly-once observation across both channels or process crashes.

# `devo-kernel`

CPython RLM REPL host (protocol v3). Spawns `python -m rlm.repl`.

## Runtime path

Set `DEVO_RLM_RUNTIME_SRC` to the directory that contains the `rlm` package,
or use the default `crates/kernel/rlm-runtime/src`.

On Windows the REPL uses an IOCP event loop with a named-pipe wakeup channel.
CPython's default loopback TCP wakeup channel can block during initialization
inside the network-restricted sandbox. The pipe keeps thread wakeups and
asynchronous subprocess I/O working without requiring network access.

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
only idle route and the pump invokes it only for `bash.completed` and the
PID-scoped `bash.consumed` withdrawal; all other idle host requests are denied.
Default deny when either handler is unset.

Background Bash may complete after its IPython cell ends. The `bash.completed`
idle route queues a follow-up while the server process is alive. Pending Bash
completion notices are process-local and are not recovered after a server
restart. Session teardown requests bounded graceful REPL shutdown; its cleanup
hook kills Bash process groups owned by that kernel. If the kernel dies
unexpectedly or graceful shutdown times out, child cleanup is best-effort, and
no completion-notice guarantee spans server or kernel loss. The
`application/vnd.devo.bash-activity+json` display marker is not part of a
client tool result and has no TUI/Desktop consumer.

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

Trace: `L2-DES-RLM-001`.

# Live Python names after compaction

Compaction does not reset a session's live Python kernel. For model requests
whose history contains a compaction summary, Devo adds a fresh, read-only sample
of the current kernel's top-level binding names. The brief is request-only: it is
not written into conversation history, a compaction checkpoint, or the reusable
model-request prefix used by approval review and Python cell-watch decisions.
Those later auxiliary requests must not replay an earlier live inventory. Resuming a
session therefore cannot replay a stale inventory as live Python state.

The brief intentionally contains **names only**, including names bound to handles.
It does not inspect values, types, properties, handle IDs/status, `repr`, or pickle
hooks. It never snapshots, prunes, deletes, restores, or starts a kernel. An
omitted name is not evidence that a binding is missing.

Limits:

- Inspect at most the first 256 namespace keys, return at most 32 names.
- Only exact built-in strings that are public ASCII identifiers of at most 64
  bytes; skip runtime bootstrap names. Sort only the bounded sample.
- Skip immediately when another kernel request owns the execution gate.
- Allow at most 200 ms for the host request. Failure, unavailable/older runtime,
  or timeout simply omits the brief, without interrupting or restarting Python.
- Ignore stdout, stderr, display, and expression-result events for this request.

The internal `namespace_brief` protocol request returns the existing `done.names`
shape. The ordinary `list_names` API is unchanged. Every subsequent model request
refreshes the brief while a compaction summary remains in history, so later
Python mutations are reflected rather than retaining an old list. This does not
guarantee a complete inventory or handle liveness; inspect a relevant binding
explicitly through Python when the task requires it.

The ready handshake advertises `supports_namespace_brief: true`. Hosts send
this request only when that capability is present; older v3 runtimes omit it
and are never probed, avoiding untagged unknown-request errors in later cells.
A timed-out or cancelled caller does not cancel an admitted stdin frame midway:
the writer retains the stdin lock until the complete frame is written.

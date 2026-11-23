# Session history and Python after compaction

## Read earlier messages and tool results

In the Python kernel, `await rlm.history.search("error text", kind="tool_result")` returns literal, case-sensitive matches on the current session's active branch, newest first. Omit the query to browse a kind. Supported kinds are `user`, `host`, `assistant`, `tool_call`, `tool_result`, `command`, `file_change`, `hosted_tool`, `plan` and `summary`. Each match has a stable `item_id`, a preview and a Unicode character offset. Use `before=next_before` to fetch older matches. The default page contains up to six matches, with a maximum of 20.

```python
hits = await rlm.history.search("connection refused", kind="tool_result")
if hits["matches"]:
    hit = hits["matches"][0]
    record = await rlm.history.read(hit["item_id"], offset=hit["offset"], max_chars=1200)
    print(record["text"])
```

Read a checkpoint's tool key with `await rlm.history.read("call:<id>")`. If several recorded calls share a key, use `search(call_id=id)` to find their individual item IDs. Pass `around=1` or `2` for neighboring previews. Reads default to 3,000 characters and allow at most 8,000. Pass `next_offset` to continue. A tool result includes its recorded call when available and reports truncation already present in the source separately from paging. Host notifications retain their provenance. Images and audio in user messages appear as attachment labels.

The host binds reads to the calling session and selects the same active transcript branch used by Native history. Durable sessions can retrieve completed records during a turn, after compaction and after reopening the session. An abandoned branch disappears from search and existing IDs on that branch stop resolving. The checkpoint includes at most six tool keys; older evidence remains searchable. The journal format and filesystem location are internal to the host.

The durable checkpoint stores the exact messages installed in the model context. Reopening a session or navigating back to an earlier checkpoint restores those messages and then appends later records on that branch. This preserves multipart user input and provider-specific reasoning attached to retained tool calls. Older checkpoints still replay from their recorded item IDs.

## Live Python names after compaction

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

# `devo-kernel-runtime`

The Python side of Devo's persistent RLM kernel.

`devo-kernel-runtime` is the child process that the Rust crate `devo-kernel`
starts with `python -m rlm.repl`. It does two jobs:

1. Run model-written Python cells in one persistent `__main__` namespace on one
   asyncio event loop.
2. Export the `rlm` namespace that cells, Python skills, and MCP integrations
   use to call back into the Rust host.

It is not the Rust host. It does not own approval policy, provider credentials,
subagent lifecycle, or shell authorization. Those belong to the host.

The Rust side calls this process the **kernel**. It is a normal CPython process,
not an operating-system kernel.

## How the host starts it

`devo-kernel` sets `PYTHONPATH` to this package's `src/` directory (or uses a
bundled copy under `python-site/`) and starts:

```text
python -m rlm.repl
```

The two processes speak newline-delimited JSON protocol v3 over stdin/stdout.
The complete wire behavior is in [`src/rlm/repl.md`](src/rlm/repl.md); the
matching Rust types are in `crates/kernel/src/protocol.rs`.

`DEVO_RLM_RUNTIME_SRC` can override the source directory during development.
The production bundle copies `src/rlm` into the application; see
`scripts/runtime/bundle.ts`.

## Architecture

```text
+-------------------------------+      JSON lines on stdin/stdout      +-------------------------------+
| Rust host (`devo-kernel`)     | <----------------------------------> | Python kernel                 |
|                               |                                      | `python -m rlm.repl`          |
| session actor, tool bridge    |                                      | one asyncio event loop        |
| approvals, credentials,       |                                      | persistent `__main__` name-   |
| subagents, MCP configuration  |                                      | space                         |
| `devo-harness` (host state)   |                                      | `rlm` API + Python skills     |
+-------------------------------+                                      +-------------------------------+
```

A session owns one kernel. Two concurrent sessions run two Python processes.
A subagent is a separate session, so it has its own kernel and namespace. The
runtime does not pool kernels.

## Runtime lifecycle

`src/rlm/repl.py` is the implementation. Its `main()` does the following:

1. `_setup_fds()` reserves a private protocol fd and redirects the process's
   normal stdout/stderr into capture pipes.
2. Starts the owner watchdog. If the Rust host exits, the kernel exits too.
3. Creates a real `__main__` module for dill snapshots and a persistent
   namespace for cells.
4. Creates one asyncio event loop. On Windows it uses the IOCP loop from
   `_windows_event_loop.py`.
5. Starts a reader thread for protocol requests.
6. Sends `{"event":"ready","protocol":3,...}`.
7. Serves requests from an asyncio queue until `shutdown` or stdin EOF.

Cells are serialized: one request at a time. Cells compile with
`PyCF_ALLOW_TOP_LEVEL_AWAIT`, so top-level `await` works. Background asyncio
tasks created by a cell keep running between cells.

`interrupt` and `host_reply` bypass the request queue. They are handled on the
reader thread because the cell that is waiting for a host reply is itself the
in-flight execute request. Queuing the reply behind that request would deadlock.

### Captured output

The kernel's OS stdout is the protocol channel, but user output must not write
plain text into it. `_setup_fds()` therefore:

- duplicates the original fd 1 into a private protocol fd;
- redirects fd 1 and fd 2 into pipes;
- replaces `sys.stdout` and `sys.stderr` with tagged writers.

Python-level writes such as `print()` become protocol events immediately:

```json
{"event":"stdout","id":"c1","text":"hello\n"}
```

Raw fd writes (`os.write`, C extensions, subprocesses, `sys.stdout.buffer`)
go through the capture pipes and arrive as `id: null` events. The runtime cannot
prove which cell wrote those bytes, so it does not attribute them.

This is why `KernelEvent` has `stdout` and `stderr` variants even though the
transport is stdout: the events preserve the logical stream and the cell
attribution. A `done` event is sent after both capture channels are drained.

## Protocol in one page

The full behavior, ordering rules, and examples live in
[`src/rlm/repl.md`](src/rlm/repl.md). The short version:

| Request | Purpose |
|---|---|
| `execute` | Run one code cell in the persistent namespace. |
| `interrupt` | Raise `KeyboardInterrupt` in a running or queued cell. No reply. |
| `host_reply` | Answer a kernel `host_request`. No reply. |
| `snapshot` | Serialize the namespace with dill and write a manifest. |
| `restore` | Load names from a snapshot. |
| `list_names` | List user-defined top-level names. |
| `shutdown` | Stop the runtime, kill local Bash children, and exit. |

| Event | Purpose |
|---|---|
| `ready` | Handshake; sent once at startup. |
| `stdout` / `stderr` | Captured logical output. |
| `result` | `repr` of a cell's trailing expression, when non-`None`. |
| `display` | MIME-keyed payload from `emit()`. |
| `host_request` | Kernel-to-host RPC. |
| `error` | An exception, with `ename`, `evalue`, and `traceback`. |
| `done` | Terminal event for one request. |

`result` is data: it is optional and appears at most once. `done` is lifecycle:
exactly one per id'd request, always last, and the collector stops there. An
`error` event is not terminal by itself; `done` follows it. `snapshot`,
`restore`, and `list_names` have no `result`; their payload is carried by
`done`.

## Module map

| Module | Role |
|---|---|
| `repl.py` | The protocol runtime: fd setup, request loop, cells, interrupts, output capture, snapshots, and `host_request`. |
| `repl.md` | Authoritative protocol behavior and wire examples. |
| `__init__.py` | Public `rlm` facade: typed host requests, file helpers, recursion API, harness proxy, lazy MCP exports. |
| `bash.py` | Host-owned, awaitable `BashHandle` and `bash()`. |
| `local_bash.py` | Explicit local shell implementation. Not the product default. |
| `shell.py` | One-shot shell command through the host approval pipeline. |
| `harness.py` | Kernel-side CRUD for persistent harness state. |
| `mcp.py` | Generic MCP facade for the Rust host. |
| `mcp_base.py` | Base class for MCP integration skill packages. |
| `skill.py` | CLI helpers for Python skill packages. |
| `windows_shell.py` | Git Bash discovery on Windows without trusting `PATH`. |
| `_winjob.py` | Windows Job Object containment for Bash children. |
| `_windows_event_loop.py` | IOCP event loop with a named-pipe wakeup channel. |
| `_snapshot_files.py` | Exclusive snapshot staging files. |
| `_snapshot_serialization.py` | Pickle filters that never reopen persisted OS file handles. |

## The `rlm` namespace

`rlm` is a namespace object, not a callable function. Use `rlm.spawn(...)`, not
`rlm(...)`. The old `rlm.run` name is deliberately gone; it raises an
`AttributeError` that points at `rlm.spawn`.

`rlm.repl` is the low-level runtime module. `rlm` is the typed facade. For
example, `rlm.repl.host_request(data)` returns the raw reply dict, while
`rlm.host_request(type, payload)` wraps the `{status, result}` envelope and
raises `RuntimeError` on failure.

The main groups are:

| API | Purpose |
|---|---|
| `rlm.bash`, `rlm.shell` | Host-owned shell execution. |
| `rlm.host_request` | Typed kernel-to-host RPC. |
| `rlm.web_search`, `rlm.web_fetch` | Host-mediated network access. |
| `rlm.update_plan`, `rlm.request_user_input` | Session UI integration. |
| `rlm.read`, `rlm.write`, `rlm.read_once`, `rlm.write_once` | Direct file I/O, with host-mediated fallback on OS denial. |
| `rlm.spawn`, `rlm.create_session`, `rlm.find_models`, `rlm.list_subagents`, `rlm.delete_subagent` | Recursive subagent control. |
| `rlm.harness`, `rlm.get_harness_state` | Cached reads and async host-owned harness CRUD. |
| `rlm.emit` | Ship a MIME-keyed display event. |

A few examples:

```python
import rlm

result = await rlm.bash("cargo test -p devo-kernel")
print(result.exit_code, result.output)

models = await rlm.find_models(limit=100)
chosen = next((m for m in models if "high" in m.thinking_levels), None)
if chosen is not None:
    reviewer = await rlm.spawn(
        "Review the current diff for concurrency bugs.",
        name="reviewer", model=chosen.selector, thinking="high",
    )
    print(reviewer.model)
```

`rlm.spawn` returns when the child is admitted, not when it finishes. The child
runs as a separate session; results arrive through `agent_message` replies or
files, usually on a later turn.

Omitted `model` and `thinking` (or `None`) inherit the immediate parent's values.
An unsupported inherited thinking level fails just like an explicit unsupported
level; there is no silent fallback. `find_models` returns enabled, configured,
routable models and supported thinking levels without credentials or network
probes. It does not guarantee provider health or credits. Selectors are exact
`provider/model` strings, and model IDs can contain slashes. `limit` must be an
integer from 1 to 100, not a boolean. See the
[child model API](../../../docs/rlm-child-models.md) for details.

`rlm.read` and `rlm.write` try the direct filesystem call first. On `OSError`
they ask the host to perform the operation through its approval pipeline. A
host denial surfaces as `PermissionError`. These helpers are an entry point, not
an OS sandbox.

## Host bridge and trust boundary

The host bridge is the package's main trust boundary.

A kernel cell calls `await rlm.host_request("bash", {...})`. The runtime emits:

```json
{"event":"host_request","id":"h1","cell_id":"c1","data":{"type":"bash","command":"cargo test"}}
```

The Rust host answers with a `host_reply` request carrying the same `id`:

```json
{"type":"host_reply","id":"h1","data":{"status":"ok","result":{}}}
```

The `cell_id` is a capability. The host only uses the host callback that was
captured for that live cell. A detached task that outlives its cell cannot
reuse a later turn's handler. Requests that arrive while no cell is active fall
back to the narrow idle Bash route or are denied.

Python is trusted code running with the user's OS privileges. The kernel
launcher does not apply an OS sandbox. Plan mode is not an OS read-only
guarantee for Python. Host-mediated actions still go through their own policy
checks. The Rust launcher strips provider credentials from the kernel
environment; it does not isolate trusted Python from files or the network.

## Bash and shell

The product path is `rlm.bash`:

```python
handle = rlm.bash("cargo test -p devo-kernel")
print(handle.running)          # pending or running
result = await handle
print(result.exit_code, result.output)
```

`BashHandle` is synchronous and non-blocking. It schedules `bash.start` through
the host and then refreshes a cached snapshot every 0.1 seconds. `handle_id` is
an opaque host handle; `pid` is always `None`. `kill()` schedules host
cancellation, including during pending approval. Awaiting the handle waits for
exit. There is no local subprocess fallback: a missing host or a denied request
is an error.

`rlm.shell` is the one-shot form. It runs one host command and returns the
structured result. `rlm.local_bash` is the explicit local implementation for
standalone use and legacy tests; it is not selected automatically and does not
provide host shell authorization.

The host owns shell selection, environment, workspace resolution, sandbox
policy, and timeout behavior. Python's `os.environ` changes are not propagated
to host commands.

## MCP integrations

There are two layers:

- `rlm.mcp` is a thin facade for the session's Rust MCP manager. Use
  `await mcp.list_tools(server)` for `{name, description, inputSchema}` entries
  and `await mcp.call_tool(server, tool, arguments)` for the complete MCP
  `CallToolResult`, including `content`, `structuredContent`, and `isError`.
  Tool-level `isError` results are returned unchanged. Host call rejections
  raise `PermissionError`; discovery, transport, and malformed-response errors
  remain visible. Neither API retries through a Python transport. An older
  host that lacks these actions must be upgraded and the session restarted.
  Connections, authentication, discovery, and approvals belong to the Rust
  host. The former kernel-local `reload`, `close`, and `McpStartupError` exports
  are removed; the session owns connection lifecycle and `list_tools` refreshes
  discovery through the host.
- `rlm.mcp_base.McpIntegration` is the base class for a Python skill package
  that exposes one MCP server as ordinary async Python methods. Tools are
  discovered on first use and bound through `__getattr__`; `call_tool(name,
  args)` is the explicit escape hatch. These authored integrations, including
  Linear and Notion, still use the Python SDK and retain their parsed-result
  contract; they are separate from the generic host facade.

The MCP exports are lazy. `import rlm` does not import the `mcp` SDK; only an
integration package that subclasses `McpIntegration` needs it.

## Continual Harness and `crates/harness`

`devo-harness` owns the persistent store. `rlm.harness` is an async Python facade over host requests; it does not read or write the state files. Rust resolves the session/global destination and checks permissions before applying a change.

### What the harness does

The Continual Harness is the agent's persistent, prompt-visible guidance and
memory. It is not the agent loop, and it does not execute tools. It answers
questions such as:

- What durable fact or preference should this session remember?
- Which reusable skill or subagent spec has the agent already defined?
- Which narrow prompt note corrects a repeated behavior?
- What changed during refinement, and why?

It stores four kinds of entries:

| Kind | Purpose |
|---|---|
| `prompt` | A narrow behavioral note added to the system prompt. |
| `memory` | A durable fact, preference, or lesson. |
| `skill` | A reusable Python skill definition with a Python `reference` and `arguments`. |
| `subagent` | A reusable delegation spec or task pattern. |

A `skill` entry is still a definition; it does not grant tools, MCP access, or
sandbox permissions. A `subagent` entry is guidance; it does not spawn a child.
Actual spawning still goes through `rlm.spawn`.

Entries are **local** to one session by default, or **global** across sessions
when written with `global_=True`. Python reserves the word `global`, hence the
trailing underscore.

A **refinement** is a small, evidence-backed edit to the harness. It can create,
update, or delete entries. For a refinement, the host plans and applies the
edits; the kernel only schedules the request.

### Host side: `crates/harness` (`devo-harness`)

The Rust crate is the host-side source of truth for the schema and the host
operations. It is linked into `devo-server`; it is not part of the Python
process. See [`crates/harness/README.md`](../../harness/README.md) for the crate
README. It owns:

- the serde types `HarnessState`, `HarnessEntry`, `HarnessKind`, `HarnessScope`,
  and `RefinementEvent`, with `SCHEMA_VERSION = 1`;
- `HarnessState::load`, `save_atomic`, and `file_path`;
- prompt digest formatting and strip/reattach helpers;
- `HarnessDigestInjector`, which loads session state, merges global entries, and
  formats the digest injected into the next turn;
- `RefineProposal` / `RefineEdit` and `apply_proposal_re_read`, which re-reads
  the file, rejects per-entry drift, applies edits, and saves;
- the append-only `refinements.jsonl` audit/undo log;
- `AutoRefineSettings` and `should_auto_refine`: root sessions default to a
  review every 25 successful user-visible turns, with a 20-minute compact
  cooldown; child sessions never auto-refine.

The host uses `crates/harness` for two jobs:

- `crates/server/src/runtime/turn_exec/query.rs` injects the digest during cold
  prompt assembly, after skills and before the goal section. Local and global
  stores are merged; on an id collision the session-local entry wins.
- The digest helpers strip existing digests from compaction input and reattach
  a fresh digest to the compacted head. They also expose the marker constants
  used by the host's history stripper.

### Kernel side: `rlm.harness`

`rlm.harness` forwards persistent operations to Rust. Mutations are awaitable; synchronous `get`, `list`, and `overview` inspect the cached view. Refresh it explicitly before reading prior state:

```python
await rlm.harness.load()
entry = await rlm.harness.create_memory(
    "Prefer targeted tests",
    "Run the smallest relevant test target before the full suite.",
)
await rlm.harness.update_memory(entry.id, entry.title, entry.content + " Keep it fast.")
await rlm.harness.delete_memory(entry.id)

await rlm.harness.create_prompt_note(
    "Patch size",
    "Do not rewrite a whole file for a small change.",
    id="patch_size",
)
await rlm.harness.create_subagent(
    "Reviewer",
    "Review one diff, list concrete defects, and cite files and lines.",
    id="reviewer",
)
```

`rlm.harness.overview()` prints a bounded summary, and
`rlm.get_harness_state(global_=True)` returns the global store explicitly.

The Python facade is long-lived and cached. Successful mutations refresh its view from the host result. There is no local filesystem fallback when the host is missing or denies a request. Existing callers must add `await` to persistent writes and use `await load()` to refresh reads.

Rust serializes host CRUD and `/refine` through a shared file transaction lock, re-reads the latest state, and atomically replaces it with a unique temporary file. New stores are private; replacement preserves existing file permissions. Plan mode rejects mutations. A session without a persistent artifact directory has an empty local read view and rejects local persistence. Explicit `HarnessState(in_memory=True)` remains available for temporary state.

### How the two sides cooperate

```text
Rust host (`devo-server`)                         Python kernel
--------------------------                         -------------
devo-harness transaction store   <── host RPC ───  rlm.harness.HarnessState
HarnessDigestInjector             ── prompt ────►  next model turn
refine.rs: schedule / plan / apply                  refine skill -> host_request
apply_proposal_re_read            ── writes ────►  harness_state.json
append_refinement                 ── writes ────►  refinements.jsonl
```

The shared paths are:

| Scope | Path | Written by |
|---|---|---|
| local | `<session_dir>/harness/harness_state.json` | host refine and host CRUD |
| global | `<devo_home>/harness/harness_state.json` | host refine and host CRUD |
| local audit | `<session_dir>/harness/refinements.jsonl` | host refinement apply |

Harness paths are resolved in Rust. The kernel still receives general runtime paths through environment variables set by
`crates/core/src/tools/handlers/ipython/startup.rs`:

| Variable | Meaning |
|---|---|
| `RLM_SESSION_DIR` | Session artifact root. |
| `DEVO_CODING_AGENT_DIR` | Devo config directory, including `auth.json`. |

### Refinement flow

1. A cell or skill calls `await refine.run(...)`.
2. The `refine` skill sends a `refine.run` `host_request`.
3. `crates/server/src/runtime/refine.rs` schedules a pending refinement. Child
   sessions cannot schedule one.
4. At the turn boundary (or immediately when the session is idle), the host
   planner produces a `RefineProposal`. It prefers the session model through
   `UsagePurpose::Refine` and falls back to a heuristic planner.
5. `apply_proposal_re_read` re-reads the target file, rejects an edit if the
   entry changed since planning, applies the proposal, saves atomically, and
   appends a record to `refinements.jsonl`.
6. The next prompt digest includes the new state. The host merges global
   entries into the local view; local entries win on an id collision.

`refine.run()` is therefore different from direct CRUD:

| API | Who plans the edit | When it applies |
|---|---|---|
| `await refine.run(...)` | Host LLM or heuristic planner | Turn boundary, or immediately when idle |
| `await rlm.harness.create_memory(...)` / `await update_*` / `await delete_*` | The caller | Immediately |
| `await rlm.harness.record_refinement(...)` | The caller | Immediately, as a history event |

Direct CRUD is useful for explicit, known facts. `refine.run()` is useful when
the agent wants the host to review evidence and decide the smallest edit.

### Failure and schema notes

- Rust's `HarnessState::load` rejects corrupt JSON. The digest injector either
  propagates that error or omits the digest, depending on the caller; it never
  invents content.
- The Python loader treats an unreadable or corrupt file as empty so a bad file
  cannot kill the REPL; the next kernel save rewrites a clean file.
- The Rust schemas are mirrored by hand. If you add or change a
  field, update both sides and bump `SCHEMA_VERSION`.
- The host's `refinements.jsonl` is an append-only audit/undo log. Python's
  `record_refinement` writes a `RefinementEvent` into the state JSON's
  `refinements` array; it does not append to the JSONL log.

## Snapshot and restore

`repl.py` implements snapshot and restore with `dill`:

- `snapshot` serializes user-defined top-level names, skips runtime names and
  `_`-prefixed names, caps the total and per-variable sizes, and writes a JSON
  manifest next to the payload.
- `restore` revives names independently and reports per-name failures.
- `_snapshot_serialization.py` rejects OS file handles anywhere in an object
  graph. Reopening a file handle from a completed write cell could truncate the
  file again; in-memory `BytesIO` and `StringIO` remain supported.
- `_snapshot_files.py` creates exclusive temporary files in the target
  directory without retrying permission denials.

The host calls snapshot after a successful cell and restores when it rebuilds a
kernel. The full behavior, including the `done` fields, is in
[`src/rlm/repl.md`](src/rlm/repl.md).

## Windows support

Windows behavior is isolated in small modules:

- `_windows_event_loop.py` replaces CPython's loopback-socket self-pipe with an
  IOCP loop and a named-pipe wakeup channel. This keeps wakeups and subprocess
  I/O working when network access is restricted.
- `_winjob.py` puts each Bash child in a kill-on-close Job Object. The child is
  created suspended and atomically inside the job, so there is no assignment
  window; descendants die when the job closes, including on kernel crash.
- `windows_shell.py` finds Git Bash from installer registrations without
  trusting a workspace `PATH`.

CI runs the Python suite on Windows. The Windows-specific tests mock the
`kernel32` boundary where the real API is unavailable.

## Development and tests

The package requires Python 3.11 or newer. Runtime dependencies are `mcp>=2,<3`
and `tyro`; `dill` is a development dependency for snapshot tests.

From this directory:

```bash
uv sync
uv run python -m unittest discover -s tests -v
```

Or without `uv`:

```bash
PYTHONPATH=src python -m unittest discover -s tests -v
```

The tests use fake host callbacks; they do not need a running Devo server. The
main test files are:

| Test file | Coverage |
|---|---|
| `test_host_bash.py` | Pending state, approval, cancellation, notices, opaque handles. |
| `test_rlm_shell.py` | Shell approval arguments and denial propagation. |
| `test_file_helpers.py` | Direct file I/O and host-mediated fallback. |
| `test_mcp_host_list_tools.py` | MCP host-only listing, calls, failures, and result contracts. |
| `test_mcp_integrations.py` | Bundled Linear/Notion discovery and parsed-result contracts. |
| `test_snapshot_files.py` | File-handle rejection in snapshots. |
| `test_snapshot_staging.py` | Exclusive staging and collision handling. |
| `test_windows_*` | Windows loop, shell discovery, and environment mapping. |

CI also runs the suite on Windows for Python 3.11 and 3.13.

## Design rules

- **Host policy stays in the host.** Python may request an action, but the host
  decides whether it is allowed.
- **No silent local fallback for host-owned actions.** If the host is missing
  or denies a request, the kernel raises an error instead of running the action
  locally.
- **Protocol framing must stay intact.** User output is captured into events;
  it never writes plain text to the protocol fd.
- **One event loop, one namespace.** Cells share state and background tasks;
  sessions and subagents do not.
- **Disk state is atomic and shared.** Harness writes use temporary files plus
  rename, and both sides tolerate the other process writing the same file.
- **Snapshots never reopen persisted OS handles.** Prefer `BytesIO`/`StringIO`
  for in-memory data.
- **Cross-platform code is isolated.** Windows behavior lives in the Windows
  modules; the common runtime stays platform-neutral.

## Glossary

| Term | Meaning |
|---|---|
| kernel / runtime | This Python process, started by `python -m rlm.repl`. |
| host | The Rust `devo-kernel` process that owns the child and the policy. |
| cell | One `execute` request and all events through its `done`. |
| protocol v3 | The newline-delimited JSON request/event format in `repl.md`. |
| `host_request` | A kernel-to-host RPC; the host replies with `host_reply`. |
| skill | A Python package imported into the kernel and exposed to the agent. |
| harness | Persistent prompt notes, memories, skills, subagent specs, and refinement history. |
| local harness | State for one session, under `<session_dir>/harness/`. |
| global harness | State shared across sessions, under `<devo_home>/harness/`. |
| refinement | A small host-planned edit to harness state, scheduled by `refine.run`. |
| digest | The formatted harness state injected into the prompt for one turn. |
| MCP | Model Context Protocol; external tools exposed through `rlm.mcp`. |
| snapshot | A dill serialization of the persistent Python namespace. |

## Where to start reading

| Order | File | Why |
|---|---|---|
| 1 | [`src/rlm/repl.md`](src/rlm/repl.md) | The wire protocol and runtime behavior. |
| 2 | [`src/rlm/repl.py`](src/rlm/repl.py) | How the runtime serves requests and runs cells. |
| 3 | [`src/rlm/__init__.py`](src/rlm/__init__.py) | The public `rlm` facade and typed host helpers. |
| 4 | [`src/rlm/bash.py`](src/rlm/bash.py) | The host-owned Bash handle. |
| 5 | [`src/rlm/harness.py`](src/rlm/harness.py) | Kernel-side harness CRUD. |
| 6 | `crates/harness/src/state.rs` | The Rust side of the harness schema and file IO. |
| 7 | `crates/harness/src/digest.rs` | Prompt digest and compaction strip/reattach. |
| 8 | `crates/harness/src/apply.rs` | Refine proposals and drift-checked apply. |
| 9 | `crates/server/src/runtime/refine.rs` | Host scheduling, planning, and apply pipeline. |
| 10 | [`crates/kernel/README.md`](../README.md) | The Rust host that starts and supervises this process. |

Related documents:

- [`crates/kernel/README.md`](../README.md) — the Rust host, sessions, and
  `host_request` routing.
- [`crates/core/src/tools/handlers/ipython/startup.rs`](../../core/src/tools/handlers/ipython/startup.rs)
  — environment variables passed to the kernel.
- [`crates/server/src/runtime/turn_exec/query.rs`](../../server/src/runtime/turn_exec/query.rs)
  — harness digest injection at turn start.

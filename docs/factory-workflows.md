# Factory workflows: experimental DAG slice

Devo Factory v1 composes ordinary direct child agents into a finite DAG.
It provides strict preflight validation, bounded parallel admission, all-parent
joins, plain-text result forwarding, explicit failure policies, local status,
and stop/cleanup. It does not replace the Rust session supervisor or grant
additional permissions.

## Dependencies and boundaries

The default backend needs the **subagent-controls implementation** providing
`rlm.collect(targets=[...], timeout_ms=0)` and its host route, in addition to the
existing `rlm.spawn` and `rlm.delete_subagent` helpers. On a baseline without
the Python collection API, `start` fails before creating a run or spawning
children. A mismatched/older host surfaces an error from its existing bridge.
The `FactoryBackend` protocol is isolated so this module and its scheduler
tests can be reviewed independently of that implementation.

Every run requires explicit `enabled=True`. This is an API opt-in, not a new
persisted session setting, permission grant, or equivalent of Prime's
`/factory on` command. Existing child model selection, resource limits,
approval rules, session ownership, and cancellation remain host-owned.

**This release advances only inside a live Python cell.** `start` validates and
registers a run without spawning; `advance` performs one pass; `wait` keeps
performing passes in that cell. There is no hidden asyncio task. Children
already spawned are host-owned and may keep running when the cell ends, but
their successors will not start until another `advance` or `wait` call.
This respects Devo's cell-bound host capability lifetime: an idle/detached
kernel request must not reuse an old or a later turn's permissions.

Run state is in kernel memory, not the durable session journal. A kernel
restart loses the graph, outputs, and cleanup tracking, not necessarily the
host-owned children. After a restart, inspect `rlm.list_subagents()` / collect
and explicitly stop unwanted children. Do not claim automatic run recovery.

## Example: parallel investigation, joined review

Run in an active Devo Python cell after the collection API is installed:

```python
spec = {
    "version": 1,
    "max_parallel": 2,
    "failure_policy": "fail_fast",
    "nodes": [
        {"id": "runtime", "prompt": "Inspect runtime behavior; report findings only."},
        {"id": "tests", "prompt": "Inspect relevant test coverage; report findings only."},
        {
            "id": "review",
            "needs": ["runtime", "tests"],
            "prompt": "Combine both findings into a concise implementation recommendation.",
        },
    ],
}
run = await rlm.factory.start(spec, enabled=True)
run_id = run["id"]
result = await rlm.factory.wait(run_id, timeout=30)
print(result)
```

If still running, use a later cell:

```python
result = await rlm.factory.wait(run_id, timeout=30)
# Or advance once, returning without waiting for child completion:
result = await rlm.factory.advance(run_id)
# Local status alone does not refresh the host or start work:
print(rlm.factory.status(run_id))
```

To stop:

```python
result = await rlm.factory.stop(run_id)
print(result)
# Only after completed / failed / stopped:
rlm.factory.forget(run_id)
```

`stop` prevents further admissions, then closes known active children using the
existing host API. Completed child transcripts are retained. Cleanup errors
remain visible under each node's `cleanup_error`, and the run remains
`stopping` rather than claiming it stopped. Resolve the host issue and retry
`stop`. A running/stopping run cannot be forgotten.

## Specification and execution semantics

- Top-level fields: `version` (exact integer 1), `nodes`, `max_parallel`
  (1–16, default 4), and `failure_policy` (`fail_fast` or `continue`).
- Nodes: unique `id` matching `[a-z][a-z0-9_-]{0,31}`, nonempty `prompt`,
  optional `needs` list, optional exact `provider/model` selector, optional
  `thinking`. Omitted model/thinking inherit the immediate parent through
  `rlm.spawn`. The host still validates the resolved model and thinking level.
- Up to 128 nodes. Duplicate IDs/dependencies, missing predecessors, cycles,
  unknown fields, and unsupported Prime machine options are rejected, never
  silently ignored. Nodes may be declared in any order.
- A node starts only after **all** dependencies have completed successfully.
  Predecessor text is appended as a JSON object keyed by predecessor ID and
  labeled as task data, not additional authorization. No eval or template
  execution occurs. A graph does not authorize its children to exceed the
  user's original task.
- Each node is admitted at most once. There are no automatic retries. Admission
  is serialized per run, including concurrent `advance`/`wait` callers.
  `max_parallel` counts admitted children, including those waiting for input.
  This does not increase the host's own global/session child limit.
- A host child requiring explicit resume is reported as `waiting_for_resume`.
  `wait` returns that actionable state immediately instead of using its entire
  polling budget. The child keeps its slot; no automatic replay occurs. Resume
  the child through the existing session controls, then advance again, or stop
  the workflow. A settled turn waiting for input also keeps its slot.
- `fail_fast` stops admission and closes remaining live children after a node
  fails. `continue` allows independent branches to finish; descendants of any
  failed node become blocked. A run with any failed node ends `failed`.
- Child `failed`, `interrupted`, `canceled`, or `closed` never becomes success.
  Missing, duplicate, or inconsistent collection replies raise without partly
  applying observations or spawning more work.
- Outputs are limited to 32,000 characters per node and the rendered successor
  prompt to 128,000 characters. Oversized content fails explicitly; it is not
  silently truncated. There are at most 32 retained runs; explicitly forget a
  terminal run to release its local state.
- `wait` timeout (0–300 seconds) is a polling budget, not a transaction deadline
  or child cancellation. An in-flight spawn/collect/close may exceed that budget
  so its acknowledgement is not lost. Interruption during spawn is treated as
  uncertain admission; the unique child name is retained for cleanup, and the
  node is never automatically respawned. If the host cannot resolve that name,
  cleanup remains unresolved and the run stays `stopping`.
- Status is a detached local snapshot: `ready`, `running`, `waiting_for_resume`, `stopping`,
  `completed`, `failed`, or `stopped`. Host state may have changed since the last
  advance; status is not a live provider-health or process-liveness promise.

## Prime comparison and next safe milestones

Reference: [Prime Factory skill at 74591cd](https://github.com/PrimeIntellect-ai/prime-agent/blob/74591cd/skills/factory/SKILL.md)
and [runtime implementation](https://github.com/PrimeIntellect-ai/prime-agent/blob/74591cd/prime-agent-runtime/src/rlm/factory.py).
Devo baseline inspected: [4228c9d](https://github.com/7df-lab/devo/commit/4228c9d).

| Capability | Prime reference | This Devo slice |
| --- | --- | --- |
| Validated definitions | Harness `factory` entries; DAG compiles to state machine | Inline versioned DAG; no persistent harness kind |
| Fan-out / fan-in | State transitions and joins | Bounded fan-out; all-dependency success joins |
| Child ownership | Ordinary supervisor-owned children | Existing Rust-owned child sessions |
| Failure control | Retries, per-state policies, escalation | Fail-fast/continue; no admission retries |
| Complex transitions | Guards, bounded cycles, foreach, residents | Rejected as unsupported |
| Run lifetime | Background kernel driver; registry lost on kernel restart | Explicit active-cell advance/wait; volatile registry |
| User controls | Persistent gate, graph/watch, stop/resume, TUI factory page | Explicit API opt-in, local status, stop/cleanup |
| Machine library | MACHINE.md seeds and import/export | Not implemented |

Next milestones should be independently reviewed: (1) integrate and exercise
the real collection API end to end; (2) design a host-owned durable run record
and bounded background scheduler with explicit permission/session lifetime;
(3) add Native-only run status/control events and UI; (4) add guarded bounded
loops, validated retry budgets, and machine-library storage. A background
Python task with a stale host bridge is not a substitute for milestone 2.

## Verification

```sh
PYTHONPATH=crates/kernel/rlm-runtime/src python -m unittest discover \
  -s crates/kernel/rlm-runtime/tests -p 'test_factory*.py' -v
```

The scheduler tests use an injected backend and cover validation, joins,
concurrency, failure propagation, malformed observations, uncertain admissions,
interruption, stop races/retries, timeouts, capacity, and output bounds. Adapter
tests verify the public helper calls. These are not a substitute for a real
provider-backed Devo session test, nor evidence of background continuation,
restart recovery, UI support, or complete Prime parity.

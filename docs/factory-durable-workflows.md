# Durable Factory workflows and guarded joins

This slice extends the experimental DAG scheduler in [Factory workflows](factory-workflows.md).
It depends on that scheduler and the RLM child collection/lifecycle controls.
Execution still advances explicitly inside active Python cells. It does not add
a background supervisor, automatic child restart, or Prime machine compatibility.

## Persist before dispatching

```python
run = await rlm.factory.start(spec, enabled=True, durable=True)
result = await rlm.factory.wait(run["id"], timeout=30)
```

`durable=False` remains the default and preserves the original volatile behavior.
For a durable run, the host saves a checkpoint before returning from `start`.
Before each spawn, it saves an `admitting` intent containing the reserved child
name. After admission, it saves the returned child ID. Observations, graph
decisions, and stop/cleanup results are checkpointed too. Unchanged observation
polls do not write a new revision. Each external mutation still checks the
current revision first, including retried cancellation.

The host resolves storage under the current session's artifact directory:
`factory/<run_id>.json`. The caller cannot supply a path, another session, or a
global scope. Records include their owning session ID and a monotonic revision.
CAS writes reject stale callers; the definition, reserved names and bound child
IDs cannot be changed, and admitted/settled nodes cannot return to pending.

Writes use a stable sidecar lock, unique temporary file, file fsync, atomic
replacement, and directory fsync on Unix. Unix files are private and operations
are relative to held directory descriptors, so a later symlink/path swap does
not redirect an approved operation. Windows directory handles disallow deletion
while the operation holds them, and reparse-point entries are rejected.

The ordinary host permission pipeline covers the store and transaction sidecars.
Plan Mode rejects saves before creating files. Read-only load/list never create
directories/locks and never load or recover child sessions. Stored content is
recovery data, not a permission grant or authoritative child lifecycle state.

Limits are 1 MiB per serialized snapshot and 128 saved runs per session, in
addition to the scheduler's limits. A size, permission, corruption, fsync, or
revision error is surfaced. There is no volatile fallback or silent truncation.
Large definitions/output can reach this bound; avoid large artifacts in prompts
and keep them in separately managed files. There is no snapshot deletion API in
this slice. `forget` drops only a terminal in-memory run, not its durable record.

## Explicit recovery

After replacing the kernel or restarting the host, first inspect the saved list:

```python
saved = await rlm.factory.saved()
print(saved)  # run_id, revision, status; no child loading or provider calls
```

Then opt in to restoring the selected run in the same parent session:

```python
run = await rlm.factory.restore(saved[0]["run_id"], enabled=True)
print(run)
# Inspect before proceeding. Only pending nodes can be newly admitted.
result = await rlm.factory.wait(run["id"], timeout=30)
```

Restore validates the checkpoint and claims a new revision before looking up
children. That claim goes through the write/Plan gate, before any lazy child
recovery can materialize journal state. It reconciles each unresolved admission
against exactly one direct child with its reserved name, and requires an already
bound child ID to match. It never spawns, replays an earlier prompt, or turns an
uncertain admission back into pending.

- A retained completed child is collected on the next advance. Its existing
  answer can unblock a successor without rerunning the completed work.
- A child that requires explicit session resume yields `waiting_for_resume` on
  advance/wait. Resume it through existing session controls, then advance again.
- A missing, ambiguous, or mismatched child leaves `recovery_required`. `wait`
  returns immediately. Inspect the host's direct children and the node error;
  do not infer that a missing acknowledgement means the child was never created.
- A lost checkpoint acknowledgement leaves `persistence_error` in the local
  view. Reload/restore rather than retrying the old revision. A persisted
  admission intent can remain unresolved even when no child actually started;
  this conservative outcome prevents duplicate work.
- Stop intent survives recovery. Further advancement closes outstanding children
  instead of dispatching successors. Failed cleanup stays visible and retryable.
- Stale in-memory schedulers cannot dispatch new work after another restorer
  claims the revision. Checkpoint failures prevent further scheduler mutations.

If persistence is unavailable while an already admitted child needs an urgent
stop, use the existing child controls directly; after storage is available,
restore and reconcile the run. Neither interruption nor a wait timeout promises
that host-owned children have stopped.

## Version 2 guards and joins

Version 1 keeps its previous schema and rejects these new fields. Version 2 adds
`when` and `join` while retaining the same finite acyclic graph and admission cap:

```python
spec = {
    "version": 2,
    "nodes": [
        {"id": "review", "prompt": 'Return JSON with an "approved" boolean.'},
        {"id": "accept", "prompt": "Summarize the approved outcome.",
         "needs": ["review"],
         "when": {"node": "review", "path": ["approved"], "equals": True}},
        {"id": "reject", "prompt": "Summarize the requested corrections.",
         "needs": ["review"],
         "when": {"node": "review", "path": ["approved"], "equals": False}},
        {"id": "summary", "prompt": "Summarize the selected branch.",
         "needs": ["accept", "reject"], "join": "any"},
    ],
}
```

Guards name an explicit dependency. Without `path`, `equals` is an exact text
comparison. With `path`, the predecessor answer must be finite JSON; the path
contains up to 16 nonempty object keys and the comparison value is a JSON scalar.
Integer comparison values must be in `[-(2**53 - 1), 2**53 - 1]` so the host and
Python preserve their type/value through checkpoint serialization.
Comparison is type-sensitive: `True`, `1`, and `1.0` are distinct. Missing paths
do not match, including when comparing to null. Invalid JSON fails the guarded
node explicitly. There is no eval, code expression, implicit prompt template,
network lookup, or authorization inherited from output text.

All joins wait for **every** dependency to settle, so branch timing does not
select a winner and no losing branch is abandoned:

- `join="all"` is the default. Failed predecessors block it; skipped predecessors
  skip it. All successful predecessors contribute their outputs.
- `join="any"` proceeds if at least one predecessor completed successfully,
  forwarding only successful outputs. If none succeeded it is skipped. With
  `failure_policy="continue"`, an independent successful branch may contribute
  even when another failed, but the overall run still reports failure.
- A false guard marks its node `skipped`, which is distinct from successful
  completion or failure. Skipped nodes spawn no child. All-skipped paths can
  complete without inventing a result.

Loops, dynamic foreach, resident agents, retry budgets, background continuation,
TUI/desktop workflow views, and a machine library remain future work.

## Verification scope

Python tests cover guarded joins, persisted admission intent, save-before-effect
failures, lost acknowledgements, stale revisions, missing/mismatched recovery,
Plan claim denial before child access, stop recovery, and corrupt snapshots.
Rust tests cover the host store, permissions, atomic/CAS behavior, filesystem
boundaries, and fresh-kernel loading. Real CPython → host → child-session tests
exercise a guarded branch/join and restoration after rebuilding the runtime.
Provider responses in these integration tests are scripted; they are not paid
model tests or evidence of a long-running background daemon.

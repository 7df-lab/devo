# Authorized trajectory refinement and rollback

## Baseline assessment

Compared at Devo `4228c9dc29439e28d1180fe5f30f1686ec4f1919` and
[prime-agent `74591cd`](https://github.com/PrimeIntellect-ai/prime-agent/tree/74591cd):

- Prime's `crates/pa-core/src/refinement/executor.rs::plan_refinement` supplies
  bounded conversation text, harness overview and prior refinement history to
  its planner. `rollback_proposal` creates inverse edits and the executor applies
  them with a planning baseline.
- Devo already had root scheduling, a model planner, scope selection, shared
  CRUD/refine locks, and atomic state-file replacement. Its planner only saw
  harness state, not the task trajectory. Its parsed Update/Delete edits had no
  baseline. `rollback_id` was written to JSONL but did not invert any edit.
- This change closes a deliberately narrower loop than Prime's broad trajectory
  review: explicitly selected evidence -> local candidate memories -> durable
  undo -> conflict-checked rollback. It is not a claim of feature parity, a
  general learning system, or a measured performance improvement.

## Native API and authorization

`session/refine/run` accepts an optional `trajectoryItemIds` array. Omitting it
preserves the existing instruction/consolidation behavior. With selected evidence,
the planner uses a fixed, constrained distillation directive rather than arbitrary
`instructions`. For example:

```json
{
  "sessionId": "<root session ID>",
  "trajectoryItemIds": ["<selected text item ID>"]
}
```

Trajectory selection requires an idle root session. Sanitized evidence is frozen
in memory at admission, so a later turn cannot expand the selected text.

A client must show the selected text for the user's review before submitting IDs,
and explain that the session's configured model/provider will receive that text
and the current target harness entries. This API field is the explicit selection
boundary, not an automatic history-mining permission. There is no new UI picker
in this change; clients may obtain IDs through the existing session item API.

- At most 16 IDs from this root session, at most 2,000 Unicode characters per
  item and 12,000 in total. Duplicate/missing IDs and unsupported types fail
  closed. Selection is read from the session actor, not arbitrary paths/journals.
- User text, assistant text and string-valued ToolResult outputs are supported.
  Reasoning, images, arbitrary structured tool output, arguments, provider
  payloads, file paths and child transcripts are not traversed or serialized.
- Trajectory selection cannot be combined with global scope, rollback, automatic
  refinement or a child session. Python/kernel `refine.run` rejects trajectory
  selection; an agent cannot opt itself into reading additional history.
- Known secret markers cause the whole matching line to be replaced before the
  model call. This is **best-effort** redaction, not a comprehensive secret/PII
  detector. User review remains necessary; do not select sensitive text merely
  because this filter exists. Text may also be truncated, explicitly marked.
- Excerpts are JSON data labelled untrusted, including historical user-role text.
  They do not authorize new instructions, permissions, preferences or policies.
  The host permits at most four local memory Create edits, with bounded text and
  no file path. Prompt/skill/subagent edits and Update/Delete are rejected.
- Candidate memories retain source item IDs and an `unverified_trajectory_lesson`
  trust label. Their digest explicitly warns that they are unverified suggestions,
  not instructions or permissions. Ordinary consolidation does not receive
  candidate entries, preventing that path from silently reclassifying them as
  skills/prompts. This is defense in depth, not a proof that
  prompt injection or hallucination is solved. Raw selected excerpts are not
  copied into a separate durable trajectory archive.
- Empty model edits remain a no-op. Invalid/failed trajectory planning does not
  fall back to copying the request into memory. Apply failures are persisted as
  `refinement_not_applied` warning items, not successful refinement items.

All manual and automatic refinement/rollback writes defer in Plan Mode. The
planner holds no store lock; host-owned baselines are captured before its model
call. Entries become available at the existing next-turn digest boundary.

## Atomic persistence and real rollback

The existing cross-process sidecar lock covers one read/check/write transaction.
`harness_state.json` now holds optional `refinement_history`, containing the
actual committed before/after images and rollback links. Entries, normal history
and undo records are serialized into one private temporary file, synced and
atomically replaced together. A later invalid edit leaves the whole previous
file unchanged. CRUD keeps this additional field intact.

New `refinements.jsonl` files are private and append-locked; existing file
permissions are preserved. The file is a compatibility audit mirror.
It is not the rollback authority. A mirror append failure cannot lose the
canonical undo record or turn a partial write into a valid history entry.

To undo an entry, send the existing Native fields:

```json
{
  "sessionId": "<root session ID>",
  "rollbackId": "<refinement ID>",
  "global": false
}
```

Rollback bypasses the model, constructs inverse Create/Update/Delete operations
from committed history, and verifies every affected current entry against the
recorded post-image before committing. Unrelated entries survive. Update versions
increase, including on undo, so restoring equal content does not erase the
history conflict. Duplicate rollback, missing/legacy-only history, stale edits,
identity mismatches, duplicate entry edits and immutable base-prompt edits fail
without partial application. Global rollback requires explicitly selecting the
global store; stored paths never redirect a write.

Old state files load with empty undo history. Old JSONL records are intentionally
not guessed into reversible changes, because they may lack baselines or may
have been appended independently of a successful state commit. An old binary
that rewrites state can drop the new optional history field; downgrade therefore
loses rollback availability, not silently reconstructs it. History is not pruned
in this minimum implementation and retains prior entry content for undo. A
privacy purge/retention UI and bounded archival need a separate design.

## Regression and evaluation boundaries

Coverage includes Create/Update/Delete undo after reload, stale forward/undo
conflicts, all-or-nothing validation, legacy-only history refusal, simultaneous
rollback, unrelated concurrent edits, monotonic versions, private files,
unsupported/duplicate/missing trajectory selection, Unicode bounds, known-secret
redaction, structural trust restrictions, digest-marker escaping, manual Plan Mode and audit-mirror
failure. Run:

```sh
cargo test -p devo-harness
cargo test -p devo-server --lib runtime::refine
cargo check -p devo-server --tests
cargo run -p devo-protocol --bin generate-native-ts -- apps/tui/src/generated/protocol
```

These are correctness regressions. They do not measure task success, latency,
cost or learning quality. No live provider benchmark, Windows crash/power-loss
experiment, or end-to-end UI selection workflow is claimed by this patch.

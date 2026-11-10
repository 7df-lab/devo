# Native message edit projection

`session/message/edit` replaces the immediately preceding user message with a new
item and turn, while retaining the original branch in the append-only journal.
The replacement inherits the target's resolved parent. An explicit root edge
(`parentId: null`) remains a root across repeated edits; it is not a request to
infer the preceding chronological item as its parent.

The server persists the replacement envelope before publishing it. The RPC result,
`item/started`, `item/completed`, and `session/items/list` use the same item ID,
sequence, revision, parent, content, and timestamps. Only the started event has a
running state. The replacement revision is the target's durable revision plus
one, including when `expectedRevision: 0` skips the concurrency check.

This keeps live transcript ordering consistent with session reopening and cold
resume. The change prevents new incorrect edges; it does not rewrite historical
journals already produced by earlier binaries.

Regression coverage: `cargo test -p devo-server --test native_message_edit_projection`.
The tests exercise repeated root edits, interruption of a busy later turn,
preservation of earlier turns, lifecycle/result/history agreement, and cold resume.

## Editing while cancellation finishes

A terminal turn event or idle status can precede the actor's `MergeTurn` and
runtime admission cleanup. After interrupting the selected turn, message edit
waits for that exact execution to leave the active-turn registry before taking
the state-change gate. This is a notification-driven wait, bounded to five
seconds in addition to the interrupt's existing terminalization timeout.

If another turn wins admission meanwhile, the guarded edit still rejects it and
does not cancel the newer turn. If cleanup does not finish, the edit returns
`ActiveTurnEditRejected` without applying the replacement; the caller can retain
its draft and retry. The edit never clears a still-running execution itself.
This does not change global terminal event timing or migrate existing journals.

Deterministic unit regressions retain an execution after terminal status to test
waiting, gate availability, newer-turn rejection, and timeout. The Native RPC
integration test additionally repeats sixteen live busy edits without retries,
checks mid-turn reads, and verifies history after a fresh-runtime resume.

A delayed scheduler from the canceled turn must also recheck runtime admission
under the gate before removing queued input. Both the post-turn chain and the
interrupt fallback leave the queue untouched if the edit replacement has already
started. Its own completion will drain that queue. Tests cover both orderings:
new queued work wins before edit, and edit wins before the old scheduler runs.

Resumed approval continuations also release admission before scheduling queued
input or goal continuation. A fresh-runtime approval recovery regression checks
that input queued during the resumed query drains after that query finishes.

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

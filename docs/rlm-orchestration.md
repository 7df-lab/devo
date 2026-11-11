# Python subagent orchestration

These operations run through the real kernel host bridge. They use existing
session actors, durable journals and Native turn admission; they do not create a
second execution engine or require model-provider network access to coordinate.

```python
worker = await rlm.spawn("Run the focused tests", name="tests")
results = await rlm.collect([worker], timeout_ms=10_000)
for result in results:
    print(result.status, result.progress_note, result.answer_text)
await rlm.end_turn()
```

## Transcript ownership

Spawning records a parent-owned Native `subAgent` item with the assigned
`agentNickname` and task. The TUI renders it as “Agent spawned”; the child's
initial task, kernel notices, tool activity and response stay in the child's
transcript and monitor. The spawn row is a historical snapshot, not a live
lifecycle indicator; `agent/list` remains the authority for current child state.
It is anchored to the parent's active or latest turn. A direct RPC spawn before
any parent turn is still listed, but does not create a synthetic transcript turn. Live and replayed item notifications route by the
owning `ItemEnvelope.sessionId`, not the sender of a host message.

Incoming host messages retain their stable `origin.sender` identity and an
optional host-resolved `origin.senderName` captured at delivery. Clients prefer
that name for message and queue labels, with the sender identity as the fallback
for older records. These messages remain non-human content, not authorization.

## Collect and progress

`collect(targets=None, *, timeout_ms=0)` accepts a child name/id, spawn handle,
subagent row, or a list/tuple mixing those selectors. Empty selection snapshots
all direct children at admission. Duplicate selectors are returned once. Foreign
sessions and unknown targets fail rather than being silently skipped.

Timeout is an integer from 0 to 300000 milliseconds. Zero is a snapshot. Positive
values wait until all selected children settle or the deadline expires, then
return current snapshots. Timeout does not cancel work. Interrupted waits fail
promptly. Waiting does not hold the session mailbox or an admission lock, does
not send steering messages, and does not consume results or other wait cursors.
The notification subscription precedes the snapshot to avoid lost wakeups.

Each `RLMChildResult` contains `rlm_child_id`, `session_name`, `session_dir`,
`status`, `settled`, `answer_text`, `answer_preview`, `answer_truncated`, `error`,
`progress_note`, and `requires_resume`. Status preserves native terminal distinctions (`completed`,
`failed`, `interrupted`, `canceled`, `closed`, `waiting_for_input`); unfinished
snapshots report `running`. Settlement waits for the completed working set to
merge and execution admission to clear, and excludes queued follow-ups and accepted mailbox messages still in transit. Results
read the latest completed turn's assistant text, bounded to 128 Ki characters
with a truncation flag and a 512-character preview. The journal retains larger
output. A resumed child never presents its previous answer as the new answer.

The host rehydrates durable direct children from the index/journal after restart.
Corrupt or missing child journals are explicit restore errors. Saved unfinished
turns, recoverable provider failures, and shutdown interruptions with no live execution report `waiting_for_resume`, `requires_resume=True`,
and `settled=False`. Collect stops waiting once every target is settled or needs
explicit Native recovery; it never silently starts a new run or replays tools.
Explicitly closing a child durably cancels recovery, including a previously
failed or interrupted turn; restarting does not make that turn resumable again.
An idle restored child with queued follow-ups also requires resume. Use Native
turn/recovery/read to identify resumable turns; approval recovery and queued-only
work use their normal approval/session resume controls. In-process
progress is intentionally transient, not recovered as if it were fresh.

Children call `await rlm.progress_note("running tests")`. Notes contain 1–512
UTF-16 code units after trimming and are accepted at most once per 10 seconds.
The result has `accepted` and an optional `retry_after_ms`. Notes appear in
`collect` and `list_subagents`, without messaging or waking the parent model.
Root calls fail. Use `agent_message` for a message that needs a response.

## End the current turn

`await rlm.end_turn()` requests normal completion at the next tool-batch boundary.
The rest of the Python cell and sibling calls in that batch still run. Tool
results are committed before completion; no additional model response is
requested. This is not cancellation and does not stop children or background
jobs. Calling it from an idle or obsolete host bridge fails. Approval-replayed
tools do not support this control until normal model execution resumes.

## Independent root sessions

```python
root = await rlm.create_session("Investigate the next issue", name="Next issue")
print(root.session_id, root.session_file)
```

`create_session(prompt, name=None, model=None, thinking=None, cwd=None)` requires
a durable root in Build mode. It creates a separately visible, durable root with
its own journal, no parent link, no inherited transcript, and no parent usage or
completion lifecycle. Normal session/list, session/resume and turn controls apply.
It is not a child and cannot be collected or deleted through subagent controls.

Model/thinking use the same exact validated resolver as spawn, with inherited
defaults and no silent fallback. Permissions and sandbox settings are installed
before the first durable metadata write and before SessionStart hooks. Initial
turn admission uses the Native handler. The handle returns only after admission,
not after the initial prompt completes. If creation succeeds but admission
fails, the error includes the existing session id; callers must not blindly retry.

For this slice, an explicit cwd must canonicalize to the caller's current working
directory. Cross-workspace roots need a separate authorization flow. Child,
ephemeral and Plan-mode callers are rejected before any root is created.

## Verification

`crates/server/tests/subagent_python_controls.rs` exercises real CPython cells,
production host dispatch, durable actor merge, nonblocking read RPCs, bounded
waits and interruption, root isolation, and child result recovery after restart.
Python wire and malformed-input cases live in `test_controls.py`.

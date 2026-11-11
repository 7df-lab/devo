# TUI shell activity

Native `task/list` aggregates two distinct shell execution owners:

- Native `command/exec` processes, from the server command-execution manager.
- Python host-owned `bash()` handles, from the core host-bash registry. These use
  `host-bash:<opaque handle>` item IDs and are scoped to their owning session.

Both are shell background-task items with command, working directory,
running/terminal state and available output. Host-bash items are read-only
observations: `executionHandle` is absent. Their IDs cannot be passed to
command-manager stdin, resize or interrupt methods. `task/read` supports these
IDs, including after Python has forgotten a completed handle.

Python normally forgets its control handle immediately after completion. A separate
bounded observation history keeps up to 64 forgotten terminal entries globally,
for at most 30 minutes after completion, with at most 16 KiB each of output and
error text. Reading this history never starts a kernel or changes shell lifetime.
Session/kernel cleanup clears that session's history and cancels its live handles.
History does not consume the live control-handle quota.

This does not introduce persistent shells or PTY support into Python host bash.
Those options remain unsupported. Foreground `rlm.shell()` calls remain ordinary
tool execution and are not represented as background shell handles.

The original turn ID is retained when the host call has one. Direct host calls
without a turn use an explicit `synthetic:host-bash:<handle>` envelope turn ID;
this is an observation placeholder, not a navigable conversation turn. Nonzero
exits map to failed tasks. Cancellation is recorded from the execution token at
completion and maps to cancelled tasks rather than inferred from output text.

## TUI activity dock

The session dock always renders Subagents, Heartbeats and Shells, including zero
counts. Focus it from the editor, select a group with arrows/Tab, and open with
Enter. Shell details are read-only and show available process output.

Heartbeat and shell catalogs refresh independently every two seconds after each
request finishes. Each result requests rendering without waiting for keyboard
input. Unavailable or failed sources are explicit, never fabricated zero counts.
Session transitions invalidate old reads; failed transitions only resume polling
when the transport still owns the displayed session. Subagents remain sourced
from the session roster and child snapshots.

`npm run test:tmux -- activity` checks real terminal boot and repeated Native
reads without sending a key, using an isolated home and a built CLI.

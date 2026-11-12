# Shell task output

Python `h = bash(command)` starts a host-owned non-interactive shell task.
`h.output()` and the awaited `BashResult.output` retain the existing bounded,
formatted output behavior, including final timeout/cancellation diagnostics.

`h.tail(n=50)` returns the latest lines from a separate rolling 256 KiB byte
buffer. It continues updating after the progress/output prefix limit is reached
and keeps the latest output after exit. The first retained line may be partial;
invalid or incomplete UTF-8 is decoded with replacement. stdout and stderr are
merged in read order, not guaranteed process write order. Empty output falls
back to executor diagnostics. Cached access is refreshed by the existing host
poll, not a new subprocess or network request.

The same recent buffer feeds the shell activity view (which has its own smaller
view limit). The rolling buffer is bounded and updated directly by pipe readers;
it does not remove progress queue limits or add an unbounded log queue.

This is not interactive stdin or a persistent terminal. `bash()` still does not
support PTY, stdin writes, service-restart recovery, or surviving its owning
kernel/session lifetime. Existing complete-output capture is separate from this
in-memory tail; the tail is not a full transcript.

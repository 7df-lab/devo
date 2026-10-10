# Native process task completion

For process tasks, Native `task/read` and `task/list` report `running` until the
command output pump has retained the terminal record and its bounded output
tail. An operating-system exit code alone does not mark a task complete: the
last output bytes may still be in transit through the process channels.

Terminal reads use the retained record even while the live entry remains during
rollout persistence. `task/list` includes that process only once. The
`command/exec/exited` notification is emitted after the record is available, so
a read triggered by the notification can observe the terminal output.

The Native wire format is unchanged. The guarantee applies to the captured tail
(up to 16 KiB), rather than an unlimited transcript. Normal exits honor the
process layer's trailing-output grace period before the pump finalizes.

Coverage includes a deterministic retained-record regression and the real
platform-specific stdin, background-process, and interruption integration
tests in `crates/server/tests/unified_exec_tool_e2e.rs`.

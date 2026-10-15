# RLM shell approval

The default RLM tool surface exposes only `ipython`. Models, including
`gpt-5.6-luna`, can request shell permissions from a Python cell using:

```python
result = await rlm.shell(
    "echo permission-probe",
    sandbox_permissions="require_escalated",
    justification="May I run this harmless probe outside the sandbox?",
)
print(result)
```

Use `sandbox_permissions="with_additional_permissions"` with
`additional_permissions` for narrow filesystem or network grants, for example
`{"network": {"enabled": True}}` or
`{"file_system": {"write": ["/absolute/path"]}}` (use native absolute paths). The request
uses the same parsing, approval UI, permission cache and shell executor as the
shell tool. An approved full escalation disables sandboxing for that command;
a narrow approval applies its overlay. The persistent Python kernel stays fenced.
Plan mode continues to deny shell mutation. A rejected approval raises an error
without executing or retrying the command locally.

Ordinary `bash()` remains a local, fenced, asynchronous process helper. Use
`rlm.shell()` when a command needs approval; do not add permission fields to the
outer `ipython` tool call. Both `import rlm; await rlm.shell(...)` and the
preloaded `rlm` namespace support the helper. Commands use the host platform shell (PowerShell on Windows); use native
syntax instead of nesting another shell. It returns the structured host
shell result, including output and status. `timeout_ms` and `prefix_rule` are
optional. File access still uses `rlm.read()` and `rlm.write()`.

## Manual verification (Windows, 2026-10-15)

Ran the product binary under psmux with `gpt-5.6-luna` and `default` permissions.
The model used `rlm.shell()` from a real Python tool cell:

- Full escalation displayed an approval prompt; choosing Yes ran the echo probe
  with exit 0 and captured its output.
- Choosing No for a second escalation returned `RuntimeError: rejected by user`;
  the model stopped and did not retry through another route.
- A narrow `file_system.write` grant displayed an approval prompt and allowed a
  native PowerShell command to create, read/assert, and delete a probe in an
  explicitly approved directory outside the workspace, with exit 0.
- Server logs recorded `state=Fenced` for the persistent Python kernel and user
  approval decisions. The test session and probe directory were cleaned up.

The initial narrow-grant command nested PowerShell with incorrect quoting. It
failed at execution; using native PowerShell syntax passed. The host's existing
PowerShell encoding prelude emits constrained-language warnings for sandboxed
commands, but the corrected probe still completed all file operations with exit 0.

## Headless execution

`devo prompt` accepts the same `rlm.shell()` helper. Its host bridge projects
`bash` and `bash.start` requests into the existing hidden shell runtime handler,
retaining permission metadata, command prefixes, and timeouts. Its existing
static-allow permission policy applies explicitly requested full or narrow grants
without marking them as human approvals. The headless command does not offer
interactive approval prompts; use the interactive client when a human approval
decision is required.

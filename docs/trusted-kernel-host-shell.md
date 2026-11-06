# Trusted Python kernel and host shell execution

The product Python kernel is a trusted, user-privileged orchestration process,
not a sandbox for untrusted Python. Direct Python filesystem, network, native
extension, and process APIs retain user permissions. Plan mode does not make
arbitrary Python OS-read-only. Host mutation checks and credential environment
filtering remain enabled; neither confines arbitrary Python.

Plan mode has no separate shell gate. Shell requests in Plan use the same
configured permission checker and sandbox profile as Build. An auto-allow does
not disable the configured sandbox. Approval-required requests still ask; a
user's denial or an existing policy denial is not converted to an allow. Plan's
no-mutation rule is model policy, not an OS read-only guarantee for Python or
shell.

Shell requests use the host session capability, approval checks, and configured
sandbox profile. A request is not a grant. Explicit escalation uses host approval.
No configured profile means no OS shell sandbox. There is no global temporary
sandbox-disable switch; tool and terminal profiles are resolved directly from
the session configuration, independently of trusted kernel startup. The kernel
launcher has no OS sandbox or filesystem grant channel; this applies to low-level
callers as well as product startup.

## Migration

Default `bash()` uses host-owned execution. It returns a pending handle before
asynchronous authorization. `handle_id` is opaque; `pid` is unavailable. Synchronous
`running`, `poll`, `output`, and `tail` read cached snapshots. `kill()` schedules
host cancellation, not an arbitrary OS signal. Python environment mutations do
not propagate to host commands. `os.getcwd()` is captured when `bash()` is called.
Host background Bash has no default timeout; explicit `timeout_ms` is honored.
Existing foreground shell tools keep their 120-second default.

The host retains at most 32 handles per session and 256 globally. Live streamed
output has a 256 KiB hard ceiling per handle. On completion, the executor-formatted
final result replaces the live buffer rather than being added to it. The default
executor truncation is `max_output_tokens` = 16,000 tokens, approximately 64,000
bytes of ASCII text (plus a truncation notice), so a normal final `poll()`/`output()`
is about 64 KiB, not 256 KiB. A caller-supplied larger `max_output_tokens`, or a
result without a text payload, can retain more up to the 256 KiB ceiling. Do not
treat 256 KiB as a guarantee of exact retained bytes. Python caches terminal
results and releases the host record; otherwise finished records expire after 30
minutes. These are new resource bounds, not the legacy local Bash buffer sizes.
Explicit local execution remains in `rlm.local_bash`; it is not an isolation
mechanism.

Pending starts stay within the creating cell's authorization lifetime. Idle
requests may inspect/cancel/release existing session-owned handles but cannot
start commands. Host work is canceled when its originating turn is canceled or
its kernel shuts down, terminates, loses its protocol stream, or is dropped.
A kernel's cancellation does not cancel replacement-kernel commands.
Interrupted first awaits request host cancellation; awaiting an
already-released background handle only waits. Cancellation while approval is
pending does not itself dismiss the approval dialog. A raw kernel interrupt can
wait for the pending admission reply; cancellation of the originating host turn
must also cancel approval. A terminal host failure without an OS exit code raises
an error; cached output remains available for inspection.

Known limitation: kernel death while an approval is pending stops owned shell
execution, but immediate cleanup of the pending approval UI/persistence is not
guaranteed. Ordinary turn cancellation uses the existing approval cleanup path.

`rlm.shell(command, workdir=...)` is the existing one-shot host path. Relative
workdir is resolved against the host session workspace. Approval and execution
use the same resolved directory. Omission uses the workspace, not Python cwd.

## Legacy Windows credential residue

Earlier Windows builds could deliver inheritable allow ACEs for a random
per-session SID and record them in
`<devo_home>/.sandbox/credential_journal.json`. This change removes the only
code that read or wrote that journal and the only code that revoked those ACEs.
Earlier builds revoked grants on a normal session shutdown, so residue is
mainly expected after crashes or interrupted shutdowns. The residue is not
cleaned automatically:

- Journal rows and their ACL entries can remain on disk after an upgrade.
- Deleting the journal file does not revoke the ACL entries. It only discards
  the record of which paths were modified.
- This change introduces no automatic ACL mutation or cleanup pass.

The SIDs are random values rather than account SIDs, but that does not make the
residue harmless: an ACE still grants access to any token that presents the
SID. Removing the residue requires an explicit migration or manual cleanup.

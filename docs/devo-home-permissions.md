# Devo home permissions

> **Temporarily disabled.** OS sandboxing and the permission picker are
> disabled repo-wide: every process runs with full user permissions and new
> sessions default to full-access. Revert by flipping
> `SANDBOX_ENFORCEMENT_TEMPORARILY_DISABLED` (Rust, `crates/sandbox/src/lib.rs`),
> `PERMISSIONS_COMMAND_ENABLED` (TUI), `PERMISSION_PICKER_ENABLED` (desktop),
> and the default-preset literals back to their asking defaults.


`DEVO_HOME` (default: `~/.devo`) is agent-owned state. Shell tools and the RLM
Python kernel can read and write it, including root-level files. The kernel
receives this grant in both Plan and Build modes; Plan still keeps the project
workspace read-only.

On Windows, the sandbox backend retains the Devo home write root instead of
filtering it out. Its own `.sandbox`, `.sandbox-bin`, and `.sandbox-secrets`
control directories retain their existing locks and cannot be granted as
independent writable roots.

Session permission changes continue to use `session/metadata/update`.
The TUI `/permissions` selector offers `default`, `autoReview`, and `fullAccess`.

## Verification (Windows, 2026-10-15)

Built with `cargo build -p devo-cli -p devo-windows-sandbox --bins`.
The 20 Windows setup tests, two kernel-fence tests, and the Plan/Build workspace
permission test passed. Rust formatting checks passed for the changed modules.

Manually launched `target/debug/devo.exe` under psmux with an explicit repository
cwd. Verified all three `/permissions` profiles and used the selector to switch
from `fullAccess` back to `default`. In the same session, two real `ipython` turns
created unique root-level probe files in `C:\Users\lenovo\.devo`, wrote and read
back a marker, and deleted the probes. The second turn ran after switching back
to `default`; its tool result reported write/read and deletion success. Server
logs confirmed the kernel launched with `state=Fenced` and completed its handshake.
The temporary psmux session was removed after verification.

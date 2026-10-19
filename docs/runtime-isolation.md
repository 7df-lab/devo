# Installed interpreter isolation

> **Temporarily disabled.** OS sandboxing and the permission picker are
> disabled repo-wide: every process runs with full user permissions and new
> sessions default to full-access. Revert by flipping
> `SANDBOX_ENFORCEMENT_TEMPORARILY_DISABLED` (Rust, `crates/sandbox/src/lib.rs`),
> `PERMISSIONS_COMMAND_ENABLED` (TUI), `PERMISSION_PICKER_ENABLED` (desktop),
> and the default-preset literals back to their asking defaults.


Release installations resolve Node and Python from the complete archive or
Devo's checksum-pinned runtime cache. They do not fall back to system
interpreters when a bundled component or cache reference is broken.

Private Node commands ignore inherited `NODE_OPTIONS` and `NODE_PATH`, so a
preload or global module setting from another application cannot prevent the
TUI from starting. Private Python commands ignore inherited `PYTHON*` settings,
then apply Devo's module path, encoding, and user-site policy. The Windows
sandbox environment snapshot follows the same Python policy. Development
binaries without a runtime manifest retain their existing host environment.

To check an installed release, launch its absolute executable path under a
real tmux/psmux TTY with system Node and Python excluded from `PATH`. Repeat
with a nonexistent `NODE_OPTIONS=--require=...` preload and invalid
`PYTHONHOME`, `PYTHONPATH`, and `PYTHONUSERBASE` values. The TUI should open,
Python cells should execute, and `sys.executable`, `sys.prefix`, and the
kernel dependency paths should point into the Devo-managed installation/cache.
Also run `devo doctor` under those settings; its runtime dependency probe must
remain healthy. These checks require real release binaries: the runtime CI
packaging fixture intentionally uses placeholder backend executables.

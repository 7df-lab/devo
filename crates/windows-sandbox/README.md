# Windows sandbox

Cross-platform API in `src/lib.rs` is used by `devo-core` (`shell_exec`) to wrap
commands on Windows when a non-`off` sandbox profile is active.

## Wired (Phase 8 — legacy restricted-token path)

Compiled behind `#[cfg(windows)]`:

| Area | Modules |
|------|---------|
| Launch argv | `launch.rs`, `request_adapter.rs`, `wrapper.rs` |
| Legacy spawn | `unified_exec/backends/legacy.rs`, `spawn_prep.rs`, `token.rs`, `process.rs`, `conpty/` |
| ACL / caps | `acl.rs`, `cap.rs`, `workspace_acl.rs`, `allow.rs`, `deny_read_*` |
| Session I/O | `pty/process.rs`, `stdio_bridge.rs` |
| Protocol adapters | `protocol/` (Devo-local permission types) |
| Setup artifacts | `setup.rs`, `identity.rs`, `dpapi.rs` (for restricted-token creds / cap SIDs) |
| Capture / preflight | `windows_impl.rs` |
| CLI early dispatch | `devo` CLI calls `run_as_windows_sandbox_if_requested` |

`prepare_windows_sandbox_launch` returns `Some(WindowsSandboxLaunch { program, args, env })`
by re-execing the current Devo binary with `--run-as-windows-sandbox` /
`--run-as-devo-windows-sandbox` and permission JSON (`WindowsSandboxLevel::RestrictedToken`).

Package `autobins = false`: elevated `setup_main` / `command_runner` under `src/bin/` are
kept for Phase 8b and are not part of the default crate build.

## Deferred (Phase 8b+)

| Area | Modules | Notes |
|------|---------|-------|
| Elevated runner | `elevated/`, `elevated_impl.rs`, `unified_exec/backends/elevated.rs` | Compiled but not selected at runtime |
| WFP network isolation | `wfp.rs`, `wfp_setup.rs` | Requires elevated setup + UAC provisioning |
| Setup / command-runner bins | `src/bin/setup_main/`, `src/bin/command_runner/` | Enable later (`autobins` / explicit `[[bin]]`) |

## Non-Windows

Darwin/Linux builds compile the stub API only (`prepare_windows_sandbox_launch` → `Ok(None)`).
Reference snapshots may remain under `src/vendor/`; active modules live in `src/`.

## Verify

```bash
cargo check -p devo-windows-sandbox -p devo-core -p devo-sandbox
rustup target add x86_64-pc-windows-msvc   # once
cargo check --target x86_64-pc-windows-msvc -p devo-windows-sandbox -p devo-core
```

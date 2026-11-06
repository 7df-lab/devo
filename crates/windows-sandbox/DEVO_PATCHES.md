# DEVO_PATCHES — divergence from upstream `windows-sandbox-rs`

This crate is a vendored fork of OpenAI codex's `codex-rs/windows-sandbox-rs`
(Apache-2.0). Baseline snapshot: v0.150.1 (2026-09 comparison). Per the project
goal, every hunk that differs from upstream after a sync must be listed here.

Sync procedure:

```sh
for f in $(ls <codex>/codex-rs/windows-sandbox-rs/src/*.rs | xargs -n1 basename); do
  diff --strip-trailing-cr <codex>/codex-rs/windows-sandbox-rs/src/$f src/$f
done
```

## Known intentional deltas (devo ahead — keep when syncing)

- `cap.rs`: `normalize_verbatim_prefix` strips `\\?\` / `\\?\UNC\` before
  `starts_with`/`components()` in `workspace_write_root_contains_path` /
  `specificity` (upstream compares raw canonicalized paths and can mis-match
  verbatim-prefixed roots).
- `allow.rs`: `path_exists_lenient`.
- `setup.rs` + `bin/setup_main/win.rs`: `#[serde(alias = "codex_home")]` and
  `CODEX_HOME` fallback for marker/payload compatibility.
- `windows_impl.rs`: WAIT_FAILED-safe wait loop; kill-on-close job.
- devo-added integration files (no upstream counterpart): `launch.rs`,
  `windows_impl.rs`, `request_adapter.rs`, `path_util.rs`, `string_util.rs`,
  `capture_stub.rs`, `otel_stub.rs`, `protocol/`, `unified_exec/`, `conpty/`,
  `pty/`, and `elevated/`.

## Kernel credential delivery removed

The Python kernel is trusted orchestration. Host-side shell execution retains
ordinary Windows sandbox restrictions. The kernel-only credential authority,
credential journal, session SID wrapper flag, and token-injection APIs have
been removed; there is no longer a long-lived sandboxed kernel identity.

## Ports applied 2026-09-19 (from upstream v0.150.1)

Step-1 batch: `audit.rs` deny-ACE error aggregation; `setup.rs`
`setup_refresh_deny_read_paths` (devo divergence: no skip-missing entries to
strip, so the upstream `remove_skip_missing_path_entries()` call is omitted).

Re-vendor batch 1 (same day):
- `acl.rs` (A5): `INHERITED_ACE` + `AceScope::{Effective,Explicit}` +
  `dacl_allow_mask_needs_refresh` + `pub path_write_aces_need_refresh`;
  `bin/setup_main/win.rs` refresh predicate now uses it (fixes perpetual
  write-ACE DACL churn on inherited `FILE_DELETE_CHILD`).
- `deny_read_resolver.rs` (A6): glob scan plans pre-validated — a
  root-anchored glob without `glob_scan_max_depth` is rejected up front
  instead of scanning unbounded from the drive root.
- `Cargo.toml`: restored upstream's `[[bin]]` declarations
  (`devo-windows-sandbox-setup`, `devo-command-runner`). The fork's
  `autobins = false` had silently disabled both helpers — provisioning could
  never find them (structural breakage, now fixed: both exes build).
  `bin/setup_main/win.rs` references fixed from `crate::*` shortcuts to
  `devo_windows_sandbox::*` + lib exports (`StatsigMetricsSettings`,
  `path_write_aces_need_refresh`).

Re-vendor batch 2:
- `helper_materialization.rs`: junction-safe helper lookup retry (canonicalize
  then retry beside the real executable).
- `setup.rs`: `SEE_MASK_NOASYNC` on ShellExecuteEx (synchronous activation on
  Tokio workers without a message loop); `DEVO_WINDOWS_SANDBOX_PROXY_PORTS`
  env wire parsed into the setup marker (setup side of the port-desync fix —
  the proxy-side publisher of this env var is P2 network-proxy work).

## Known upstream-behind gaps (to close on next sync — see design doc)

- Proxy-side publisher of `DEVO_WINDOWS_SANDBOX_PROXY_PORTS` (P2) and
  `WindowsSandboxProvisioningSettings` plumbing.
- `setup.rs`: no-reparse HANDLE for ProvisionOnly DACL mutation;
  symbolic-root read gating (needs protocol `missing_path_behavior` /
  `has_symbolic_root_read_access` first); deny-read-key read-root filtering.
- `identity.rs` drift is test-only (no functional gap).

# Desktop development

From `apps/desktop`, run `bun run dev`. This regenerates Native protocol
types, builds the Rust runtime, and starts Electron with the renderer dev
server. `bun run preview` uses the same runtime preparation for a preview
of the production renderer.

## Windows executable locks

Windows prevents Cargo from replacing an executable while it is running.
Desktop development therefore builds the CLI and Windows sandbox helpers
in `target/desktop-dev`, rather than `target/debug`. If `CARGO_TARGET_DIR`
is set, the desktop build uses a `desktop-dev` subdirectory there.
The first build in this directory needs to compile its own dependencies.

Each Electron launch receives a separate temporary copy of the CLI,
sandbox helpers, and runtime DLLs through `DEVO_DESKTOP_DEVO_BIN`.
Rebuilding with `bun run dev:cli` while the app is running leaves that
instance on its current runtime. Restart `bun run dev` to use the new build.
Existing CLI sessions using `target/debug/devo.exe` can remain open.

An explicit `DEVO_DESKTOP_DEVO_BIN` override still selects that runtime.
Temporary copies are removed when the launcher exits normally. An abnormal
termination or a surviving child process can leave a temporary
`devo-desktop-dev-*` directory until the process exits and it can be removed.
Packaged applications continue to use their bundled runtime.

## Removing a project folder

Removing a folder from Desktop interrupts its active sessions, discards their
queued and steering inputs, and permanently deletes their histories. The
workspace directory and its files remain on disk. Other project folders and
their sessions remain available.

The renderer deletes sessions with up to eight concurrent requests. The
runtime cancels a session's subagent tree together, allows a 500 ms turn
finalization window, then aborts unresponsive turns. Kernel shutdown also has
a 500 ms graceful window before forced termination. Background commands are
stopped across connections even when their session has no active model turn.
Confirmed deletions leave the sidebar immediately. Failed deletions retain
the folder and its connection so removal can be retried.

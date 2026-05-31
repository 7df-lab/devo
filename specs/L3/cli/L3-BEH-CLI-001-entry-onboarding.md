---
artifact_id: L3-BEH-CLI-001
revision: 3
status: Draft
active_baseline: no
---

# L3-BEH-CLI-001 — CLI Entry Point and Onboarding

## Purpose

Define the concrete behavior for the CLI entry point: argument parsing, server lifecycle management, TUI launch, onboarding flow for missing configuration, and process exit handling.

## Source Design

L2-DES-APP-007 (CLI Onboarding Entry), L3-BEH-APP-001 (Configuration Resolution And Persistence), L2-DES-APP-003 (Client Server Protocol), L1-REQ-MODEL-003 (Onboarding)

## Behavior Specification

### B1. Argument Parsing

- **Trigger**: User runs `devo` in the terminal.
- **Preconditions**: The binary is installed and in PATH.
- **Algorithm / Flow**:
  1. Parse CLI arguments using `clap`:
     - `[workspace]` (positional, optional): path to the workspace root. Defaults to current working directory.
     - `--model <MODEL>`: override model selection for this session.
     - `--permissions <PROFILE>`: override permission profile.
     - `--config <PATH>`: path to a custom config file.
     - `--onboard`: start in onboarding mode before normal session interaction.
     - `--session <SESSION_ID>`: resume a specific session.
     - `--new-session`: force a new session even if one exists.
     - `--server-only`: start the server without launching the TUI.
     - `--debug`: enable debug logging.
     - `--version`: print version and exit.
     - `--help`: print help and exit.
  2. If `--version` or `--help`: print and exit immediately (no server startup).
  3. Resolve the workspace root to a canonical absolute path. If the path doesn't exist: error "Workspace not found: <path>".
- **Postconditions**: CLI flags are parsed. Workspace is validated.

### B2. Onboarding Check

- **Trigger**: After argument parsing, before server start.
- **Preconditions**: Config directories exist.
- **Algorithm / Flow**:
  1. Load effective configuration through the configuration resolver defined by `L3-BEH-APP-001`.
  2. Check if at least one `ModelProviderBinding` is in a valid state (provider exists + has credentials + supported model is known).
  3. If `--onboard` is present: launch onboarding TUI flow (B3), even when a valid binding already exists.
  4. If no valid bindings:
     a. Print: "Welcome to Devo! No model provider configured."
     b. Launch the onboarding TUI flow (B3).
  5. If valid bindings exist and `--onboard` is absent: proceed to server lifecycle (B4).
- **Postconditions**: Either onboarding runs or the main TUI launches.

### B3. Onboarding Flow

- **Trigger**: No valid model bindings exist, or user runs `devo --onboard`.
- **Preconditions**: Terminal is interactive (stdin is a TTY).
- **Algorithm / Flow**:
  1. Start the TUI onboarding mode defined by `L3-BEH-TUI-005`.
  2. The onboarding UI must run the model-first flow from `L2-DES-TUI-001`: model slug selection, provider selection or provider creation, provider name/base URL/API key, provider model name, display name, invocation method, and reasoning effort where supported.
  3. On confirmation:
     a. Persist non-secret configuration to the selected `config.toml` source through `L3-BEH-APP-001`.
     b. Persist credential material to the user-scoped `auth.json` source through `L3-BEH-APP-001`.
     c. Create or update the provider and model binding records.
     d. Set the binding and reasoning defaults required by onboarding.
  4. After onboarding completes: proceed to server lifecycle (B4).
  5. If the user cancels onboarding (Esc, Ctrl+C): exit with message "Setup incomplete. Run 'devo --onboard' to finish configuration."
- **Postconditions**: At least one valid model binding exists. Configuration is persisted.

### B4. Server Lifecycle Management

- **Trigger**: Onboarding is complete or skipped (valid bindings exist).
- **Preconditions**: Workspace is valid. Config is loaded.
- **Algorithm / Flow**:
  1. Check for an existing server instance (read endpoint descriptor). If found and responsive: reuse it.
  2. If no server exists: start the server as a detached child process:
     - Fork/exec the same binary with `--server-only --runtime-dir <dir>`.
     - Pass the config path, workspace root, and permission profile as args or env vars.
     - Wait for the endpoint descriptor to appear.
  3. If `--server-only` flag was passed: the process IS the server. Run the server main loop (WebSocket listener, execution engine). Do not launch TUI.
  4. Otherwise: launch the TUI client, connecting to the server's WebSocket endpoint.
- **Postconditions**: Server is running. TUI is connected (unless `--server-only`).

### B5. Signal Handling and Clean Exit

- **Trigger**: Process receives SIGINT (Ctrl+C) or SIGTERM.
- **Preconditions**: Server and/or TUI are running.
- **Algorithm / Flow**:
  1. On SIGINT (first press):
     - If TUI is connected: send `turn/interrupt` for the active turn (if any). Do NOT exit.
     - If TUI is not connected (server only): initiate graceful shutdown.
  2. On SIGINT (second press within 2 seconds): force quit. Terminate server process, restore terminal.
  3. On SIGTERM: initiate graceful shutdown:
     - Server: complete in-progress durable writes, close WebSocket connections, remove endpoint descriptor, exit.
     - TUI: disconnect from server, restore terminal, exit.
  4. On TUI exit (Ctrl+D or `/exit`): disconnect from server. Do NOT kill server by default (it may have other clients).
  5. The server's child process should exit when the parent TUI process exits IF the server was started by this TUI and has no other clients. Implement via a parent-pid watch.
- **Postconditions**: Terminal is restored. No orphaned processes leak. Server persists durable state.

### B6. Error Exit Codes

- **Trigger**: CLI encounters a fatal error.
- **Preconditions**: The error is classified.
- **Algorithm / Flow**: Exit with consistent codes:
  - `0`: Success (normal exit).
  - `1`: General error (config parse failure, invalid args).
  - `2`: Server connection failed (server not found, reconnection exhausted).
  - `3`: Authentication failed (invalid API key, provider error 401/403).
  - `4`: Workspace not found or inaccessible.
  - `5`: Onboarding incomplete (user cancelled setup).
  - `130`: Interrupted by SIGINT (128 + 2).
- **Postconditions**: Scripts and automation can check exit codes.

## Traceability

| L2 Source | Relationship |
|---|---|
| L2-DES-APP-007 | specified-by |
| L2-DES-APP-002 | specified-by |
| L2-DES-APP-005 | specified-by |
| L3-BEH-APP-001 | related-to |
| L2-DES-APP-003 | specified-by |
| L1-REQ-MODEL-003 | specified-by |

## Implementation Placement Guidance

- The CLI entry point belongs in the CLI crate; `crates/cli/src/main.rs` is a conventional placement. Argument parsing may use `clap`.
- Server process management: on Unix, use `fork()` + `exec()` or `std::process::Command::spawn`. On Windows, use `CreateProcess` via `std::process::Command`.
- API key storage: store only in the user-scoped `auth.json` with user-only file permissions where the platform supports them. The designed durable credential storage path is `~/.devo/auth.json` or the Windows equivalent, not workspace files, environment variables, keychains, or external secret stores.

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-27 | Assistant | Initial | Initial CLI entry and onboarding behavior. |
| 2 | 2026-05-27 | Assistant | Correction | Aligned manual onboarding with `devo --onboard`, delegated the model-first onboarding flow to the TUI L3, and corrected configuration/credential persistence references. |
| 3 | 2026-05-27 | Assistant | Correction | Clarified that onboarding credentials are persisted only to user-scoped `auth.json`, even when non-secret config is written to workspace scope. |

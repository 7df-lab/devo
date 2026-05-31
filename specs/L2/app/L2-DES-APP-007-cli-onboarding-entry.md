---
artifact_id: L2-DES-APP-007
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-27
---

# L2-DES-APP-007 - CLI Onboarding Entry

## Purpose

Define how users explicitly enter onboarding through startup CLI arguments rather than a TUI slash command.

## Background / Context

Onboarding is a setup workflow that may collect provider credentials, write configuration, and determine whether normal model invocation can begin. It should be available before or during TUI startup, but it should not be exposed as a runtime slash command inside an active session.

The TUI onboarding screens remain defined by `L2-DES-TUI-001`. This document defines only the application startup entry point that chooses to start those screens.

## Source Requirements

- `L1-REQ-MODEL-003` requires onboarding for first-time use and missing required configuration.
- `L1-REQ-TUI-010` requires TUI onboarding for required model setup.
- `L1-REQ-TUI-006` requires command discovery but excludes onboarding from slash-command entry.
- `L1-REQ-APP-010` requires onboarding-entered configuration to be persisted.

## CLI Arguments

The CLI should provide an explicit onboarding argument:

```text
devo --onboard
```

Behavior:

- `--onboard` starts the program in onboarding mode.
- If the TUI client is launched, it enters the onboarding UI defined by `L2-DES-TUI-001` before normal session interaction.
- The onboarding flow must not require the user to type a slash command after launch.
- Completing onboarding writes configuration through the precedence and persistence rules defined by `L2-DES-APP-002` and `L2-DES-APP-005`.
- Canceling onboarding exits onboarding mode and should return to a safe startup outcome: either normal startup if required configuration exists, or a clear message that required configuration is still missing.

Optional future CLI arguments may select the non-secret configuration persistence target, such as user-scoped or workspace-scoped configuration. Until such an argument is specified, default target behavior remains owned by `L2-DES-APP-002`. Credential material still writes only to user-scoped `auth.json`.

## Automatic Onboarding

The program may still enter onboarding automatically when required model configuration is missing.

Startup decision order:

```text
CLI args
    ↓
If --onboard is present: start onboarding
    ↓
Else load effective configuration
    ↓
If required model configuration is missing: start or offer onboarding
    ↓
Else start normal session UI
```

This preserves first-run usability while keeping manual onboarding entry outside slash-command discovery.

## TUI Boundary

The TUI slash-command catalog must not include `/onboard`.

If the user is already inside an active TUI session and needs to repair or add model configuration, `/model` remains the session-local model setup and selection workflow. Full onboarding is a startup/setup mode entered before normal interactive session work.

## Error And Recovery Behavior

- If `--onboard` is used while another server instance is already running, the client/server startup flow should connect to the server only if it can safely enter onboarding mode without disrupting active work.
- If active work prevents onboarding, the program should report a clear startup error or open a safe read-only status view rather than injecting onboarding into the active session.
- If persistence fails, onboarding remains in a recoverable state and reports the target configuration source.
- If required configuration remains incomplete after cancelation, normal model invocation must not begin silently.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-MODEL-003 | 1 | specs/L1/L1-REQ-MODEL-003-onboard.md | Defines explicit CLI entry for the onboarding flow required by model setup. |
| related-to | L1-REQ-TUI-010 | 1 | specs/L1/L1-REQ-TUI-010-onboarding-ui.md | Starts the TUI onboarding UI without requiring a slash command. |
| related-to | L1-REQ-TUI-006 | 1 | specs/L1/L1-REQ-TUI-006-command-discovery-control.md | Removes onboarding from runtime slash-command discovery. |
| related-to | L1-REQ-APP-010 | 1 | specs/L1/L1-REQ-APP-010-configuration.md | Onboarding writes durable configuration through application configuration rules. |
| related-to | L2-DES-TUI-001 | 1 | specs/L2/tui/L2-DES-TUI-001-onboarding-ui-flow.md | The CLI entry starts the TUI onboarding flow defined there. |
| related-to | L2-DES-APP-002 | 2 | specs/L2/app/L2-DES-APP-002-configuration-precedence.md | Defines persistence target and source precedence for onboarding output. |
| related-to | L2-DES-APP-005 | 2 | specs/L2/app/L2-DES-APP-005-config-toml-schema.md | Defines persisted config and credential schema written by onboarding. |
| specified-by | L3-BEH-CLI-001 | 3 | specs/L3/cli/L3-BEH-CLI-001-entry-onboarding.md | L3 defines CLI argument parsing, onboarding entry, server lifecycle, signal handling, and exit behavior. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-27 | Assistant | Initial | Initial CLI onboarding entry design using `--onboard` and excluding onboarding from TUI slash-command discovery. |

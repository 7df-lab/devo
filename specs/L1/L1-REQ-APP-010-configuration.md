---
artifact_id: L1-REQ-APP-010
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Human
last_updated: 2026-05-22
---

# L1-REQ-APP-010 — Configuration

## Purpose

Let users control program defaults and preferences across sessions and clients.

## Why This Matters

Configuration turns repeated preferences into durable behavior. Users need predictable defaults for models, permissions, tools, integrations, interface behavior, and telemetry without restating them every session.

## Background / Context

Users need control over modes, permissions, tools, integrations, interface behavior, logging, and telemetry. Some tools require their own execution configuration, including web search.

Model and reasoning defaults are configured through onboarding and supported model-selection workflows, rather than requiring a separate generic settings screen for reasoning effort changes.

Configuration is loaded from a user-scoped configuration file and, when present, a project-scoped configuration file. The project-scoped configuration file at `project_directory/.dev/config.toml` takes precedence over the user-scoped configuration file. The user-scoped configuration file is `C:\Users\username\.devo\config.toml` on Windows and `~/.devo/config.toml` on macOS or Linux.

## User / Business Requirement

The program must provide persistent application-level configuration for core user-facing behavior.

## Real User Scenarios

- A user sets a preferred model and expects future sessions to use it by default.
- A user changes pending model or reasoning before the first message and expects that selection to become the default for future sessions.
- A user changes reasoning during an active session and expects later turns in that session to keep using the new reasoning effort.
- A user exits the server after changing reasoning in the current session and expects that reasoning effort to be restored as the default on the next launch.
- A user selects Security Mode before sending the first message for an authorized engagement and expects that pending selection to persist as the default mode for future sessions.
- A user disables telemetry and expects that setting to apply across restarts and client surfaces.
- A user chooses whether web search should use a cloud-based provider search service or a locally configured search path.

## Functional Requirements

- The user must be able to configure default model and reasoning settings through onboarding and supported model-selection workflows.
- Configuration information entered during onboarding must be persistently saved to a configuration file.
- Onboarding-created model, provider, provider-specific model name, invocation method, and reasoning effort configuration must be restorable in later launches.
- The program must support a project-scoped configuration file at `project_directory/.dev/config.toml`.
- The program must support a user-scoped configuration file at `C:\Users\username\.devo\config.toml` on Windows.
- The program must support a user-scoped configuration file at `~/.devo/config.toml` on macOS and Linux.
- When both project-scoped and user-scoped configuration files exist, project-scoped configuration must take precedence over user-scoped configuration for overlapping settings.
- When configuration is persisted, the program must make the persistence target deterministic so the user can understand whether the saved value is project-scoped or user-scoped.
- The program must not require a separate generic settings screen as the post-onboarding path for changing model reasoning effort.
- Before the first user message is sent, changing the pending model or reasoning selection must automatically persist that selection as the default model configuration where supported.
- After the first user message is sent, changing model or reasoning selection must update the current session selection and continue to apply to later turns in that session.
- When the server exits gracefully, the program must persist the current active session reasoning effort as the default reasoning configuration for future sessions.
- The user must be able to configure or inspect the default operating mode where mode defaults are supported.
- Before the first user message is sent, changing the pending mode selection must automatically persist the selected mode as the default mode configuration.
- After the first user message is sent, the user must not be able to change the active mode of that session through configuration.
- The user must be able to configure the default permission policy and sandbox behavior.
- The user must be able to configure tools, skills, MCP sources, theme, keybindings, logging, and telemetry preferences.
- The user must be able to configure tool execution options where tools require them, including the effective web search execution path.
- The user must be able to inspect the currently effective configuration.

## Non-Functional Requirements

- Configuration errors must be actionable.
- Configuration must be durable across normal application restarts.

## Acceptance Criteria

- Given a changed configuration value, when the user starts a later session, then the new value is applied.
- Given onboarding completes with model provider information, when the program restarts, then the onboarding-entered configuration is loaded from persistent configuration without requiring the same setup again.
- Given both `project_directory/.dev/config.toml` and the user-scoped configuration file define an overlapping setting, when the program computes effective configuration, then the project-scoped value takes precedence.
- Given no project-scoped configuration file exists, when the user-scoped configuration file exists, then the program can load applicable settings from the user-scoped configuration file.
- Given configuration is persisted from onboarding or model selection, when the user inspects effective configuration, then the user can understand the scope or source of the saved value where that distinction affects behavior.
- Given the first user message has not been sent, when the user changes pending model or reasoning selection, then the selected value is persisted as the default model configuration where supported.
- Given the first user message has been sent, when the user changes model or reasoning selection in that session, then later turns in the same session use the changed selection.
- Given the server exits gracefully after the current active session reasoning effort changed, when the program next starts, then that reasoning effort is available as the default reasoning configuration.
- Given an invalid configuration, when the program loads it, then the user receives a specific error and recovery path.
- Given a setting is overridden for one turn, when a later turn starts, then the user can distinguish temporary override from persistent configuration.
- Given multiple client surfaces are used, when the effective configuration is inspected, then the same shared defaults are visible where applicable.
- Given web search has configurable execution paths, when the user inspects configuration, then the active web search path is visible.
- Given a mode is active or configured as default, when the user inspects effective configuration, then the mode and mode-specific effects are visible.
- Given the first user message has not been sent, when the user changes the pending mode selection, then the selected mode is persisted as the default mode configuration.
- Given the first user message has been sent, when configuration changes the default mode, then the existing session's active mode is unchanged.

## Out of Scope

- The program does not define the full TOML schema, field-level merge algorithm, or configuration UI layout in this L1 requirement beyond the required configuration file locations and project-over-user precedence.
- This requirement does not define exact conflict behavior when multiple active sessions have different reasoning efforts at server exit.
- This requirement does not require every setting to be configurable from every client surface.

## Open Questions

- Which settings should be allowed as per-turn overrides?
- Should default mode be global, workspace-specific, or selected explicitly before the first user message?
- Which tool-specific settings should be global, workspace-specific, session-specific, or per-turn overrideable?
- If multiple sessions are active when the server exits, which session's reasoning effort should become the persisted default?
- Should the user be able to override the default onboarding persistence target when both user-scoped and project-scoped configuration files are writable?

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refined-by | L2-DES-APP-002 | 1 | specs/L2/app/L2-DES-APP-002-configuration-precedence.md | L2 defines configuration source precedence, effective configuration resolution, and onboarding persistence. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-20 | Assistant | Initial | Initial draft with approved L1 refinement. |
| 1 | 2026-05-21 | Human | Refinement | Added tool-specific configuration requirements including web search execution path. |
| 1 | 2026-05-21 | Human | Refinement | Added operating mode configuration visibility. |
| 1 | 2026-05-21 | Human | Refinement | Clarified that mode defaults do not change the active mode of existing sessions. |
| 1 | 2026-05-21 | Human | Refinement | Added pre-first-message mode changes and automatic default-mode persistence. |
| 1 | 2026-05-21 | Human | Refinement | Clarified that pre-first-message mode changes apply to pending mode selection because the session is created by the first user message. |
| 1 | 2026-05-22 | Human | Refinement | Clarified model and reasoning default behavior: onboarding and model-selection workflows configure defaults, active-session changes are sticky, and graceful server exit persists the current active session reasoning effort. |
| 1 | 2026-05-22 | Human | Refinement | Added onboarding configuration persistence and project-over-user configuration file precedence. |
| 1 | 2026-05-25 | Human | Refinement | Renamed configurable approval posture to permission policy and kept sandbox behavior as a separate configuration concern. |

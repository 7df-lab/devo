---
artifact_id: L2-DES-APP-002
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-25
---

# L2-DES-APP-002 — Configuration Precedence

## Purpose

Refine configuration requirements into a source-precedence and persistence design for user-scoped and project-scoped configuration.

## Background / Context

The program has durable configuration at two scopes. User-scoped configuration carries personal defaults across projects. Project-scoped configuration carries settings that should apply when working inside a specific project directory.

Onboarding creates durable model invocation configuration. That setup can include user providers, provider credential references, provider-specific model names, invocation methods, and reasoning effort defaults. Credential material such as API keys is saved in the companion `auth.json` file for the same source scope. These values must be saved before onboarding is considered complete.

Post-onboarding model selection also interacts with configuration, but not every model selection should rewrite configuration files. The design must distinguish durable provider and binding records from current-session model selection and persisted default selection.

The concrete `config.toml` and `auth.json` file schemas for these configuration sources are defined by `L2-DES-APP-005`.

## Source Requirements

- `L1-REQ-APP-010` requires persistent configuration, specific configuration file locations, and project-over-user precedence.
- `L1-REQ-MODEL-001` requires persisted invocable model configuration.
- `L1-REQ-MODEL-002` requires persisted provider configuration.
- `L1-REQ-MODEL-003` requires onboarding-created model and provider configuration to be restorable.
- `L1-REQ-TUI-010` requires the TUI to submit successful onboarding results for persistence.
- `L1-REQ-TUI-006` requires slash commands such as `/model` to provide user-facing control surfaces.
- `L1-REQ-APP-012` requires safe credential handling and routine client views that do not expose plaintext credentials by default.

## Design Requirement

The program should compute an effective configuration from available configuration sources while preserving source identity for diagnostics and inspection.

Configuration source priority is:

1. Project-scoped configuration: `project_directory/.dev/config.toml`, with credentials in `project_directory/.dev/auth.json`
2. User-scoped configuration:
   - Windows: `C:\Users\username\.devo\config.toml`
   - Windows credentials: `C:\Users\username\.devo\auth.json`
   - macOS and Linux: `~/.devo/config.toml`
   - macOS and Linux credentials: `~/.devo/auth.json`

When both sources define overlapping settings, the project-scoped value takes precedence.

## Effective Configuration

Effective configuration is resolved conceptually as:

```text
User config
        +
Project config
        ↓
EffectiveConfig
```

Resolution rules:

- Missing configuration sources are allowed.
- User-scoped configuration provides the base values.
- Project-scoped configuration overlays user-scoped configuration for overlapping settings.
- Non-overlapping settings from both sources may contribute to the effective configuration.
- Effective configuration should retain enough source metadata to explain which source supplied a value when users inspect configuration or when errors occur.
- Invalid higher-priority configuration should produce an actionable error instead of silently falling back to lower-priority configuration for the same setting.

For keyed collections such as providers or model-provider bindings:

- Stable identifiers are used to detect overlapping records.
- A project-scoped record with the same stable identifier as a user-scoped record overrides the user-scoped record.
- User-scoped records that do not overlap project-scoped records remain available unless project-scoped configuration explicitly disables or replaces the relevant collection according to a later schema rule.

For selected defaults:

- A project-scoped default model binding overrides a user-scoped default model binding.
- A project-scoped default reasoning effort overrides a user-scoped default reasoning effort for the same effective binding.

## Onboarding Persistence

Successful onboarding model setup produces durable configuration data:

- Selected supported model slug.
- Selected existing provider or newly created provider.
- Provider name, base URL, and credential reference when a provider is added.
- Credential material in `auth.json` when a provider credential is added or updated.
- Provider-specific model name.
- Invocation method.
- Reasoning effort when the selected supported model permits reasoning.
- Default binding or default reasoning selection where required by the onboarding flow.

The program should persist onboarding output before normal model invocation begins. If persistence fails, onboarding should report a recoverable configuration error rather than allowing the user to believe setup is durable.

Until a dedicated target selector is specified, the default persistence target should be deterministic:

- If onboarding runs with an active project directory, persist to `project_directory/.dev/config.toml`.
- If onboarding runs without an active project directory, persist to the user-scoped configuration file for the current operating system.

When the persistence target affects visibility, sharing, or credential placement, the program should make the target understandable to the user through confirmation, inspection, or error output. In particular, project-scoped credential persistence means writing to `project_directory/.dev/auth.json`, not to `project_directory/.dev/config.toml`.

## Model Selection Persistence

The program should treat model selection state and durable model configuration as separate concerns:

- Creating or updating a provider or model-provider binding is a configuration write.
- Selecting an already-configured binding for a running session is session state, not a provider or binding rewrite.
- Persisting a default selected binding or default reasoning effort is a default-selection write, not a provider or binding rewrite.

Before the first user message in a session, changing the pending model or reasoning selection should persist the selected default where supported by `L1-REQ-APP-010`. That write should update only default-selection fields unless the workflow also created or repaired provider or binding records.

After the first user message in a session, changing model or reasoning selection should update the current session selection and should not immediately rewrite provider records, binding records, or default-selection fields. Graceful server-exit persistence of active reasoning effort is an application lifecycle policy and should update only the relevant default reasoning field.

When `/model` or another post-onboarding flow creates a provider or model-provider binding, the new or modified durable records must be persisted before the new binding is treated as configured for later launches. If the same flow also selects the binding for the current session, session selection should be applied only after the configuration write succeeds or after the user explicitly chooses a recoverable non-durable path in a later design.

## File Write Safety

Configuration file persistence should be schema-aware and conservative:

- Writes must preserve unrelated configuration keys and sections.
- Writes should avoid rewriting provider or binding records that did not change.
- Writes should validate the resulting configuration before replacing the effective file contents.
- Failed writes must not leave a partially written configuration file.
- Errors must identify the intended configuration target and affected setting or record without printing plaintext credentials by default.
- If parent directories or files must be created, creation should follow the same user-scoped or project-scoped target rules used for the write.
- Credential value writes must target `auth.json`, while non-secret configuration writes must target `config.toml`.
- A provider, binding, or MCP server config write must not leave `config.toml` referencing a missing credential id after a failed `auth.json` write.

The TOML and auth JSON section and field schemas are defined by `L2-DES-APP-005`. Comment preservation behavior, locking strategy, atomic write mechanics, and concurrent edit handling belong in L3 design or implementation design.

## Credential Handling

Credential entry during onboarding is an explicit credential-handling flow. Persistent `config.toml` records store credential references only. Credential material is stored in the companion `auth.json` file for the selected source scope.

Rules:

- Routine client model lists, provider lists, and model switchers should show credential status rather than plaintext credential values by default.
- Errors should identify the affected provider and configuration source without printing plaintext credentials by default.
- The program should not recommend environment variables, OS keychains, or external credential stores as the durable credential persistence path.
- If credential material is stored in project-scoped `auth.json`, the program should make the project-scoped persistence target understandable to the user.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-APP-010 | 1 | specs/L1/L1-REQ-APP-010-configuration.md | Defines configuration sources, precedence, and persistence target behavior. |
| related-to | L1-REQ-MODEL-001 | 1 | specs/L1/L1-REQ-MODEL-001-config.md | Model provider bindings are durable configuration records. |
| related-to | L1-REQ-MODEL-002 | 1 | specs/L1/L1-REQ-MODEL-002-provider.md | Provider records are durable configuration records. |
| related-to | L1-REQ-MODEL-003 | 1 | specs/L1/L1-REQ-MODEL-003-onboard.md | Onboarding creates configuration that must be persisted. |
| related-to | L1-REQ-TUI-010 | 1 | specs/L1/L1-REQ-TUI-010-onboarding-ui.md | TUI onboarding submits setup results for persistence. |
| related-to | L1-REQ-TUI-006 | 1 | specs/L1/L1-REQ-TUI-006-command-discovery-control.md | Slash commands can trigger session selection or configuration writes. |
| related-to | L1-REQ-APP-012 | 1 | specs/L1/L1-REQ-APP-012-privacy-data-ownership.md | Credential persistence and projection behavior must follow privacy expectations. |
| related-to | L2-DES-APP-005 | 1 | specs/L2/app/L2-DES-APP-005-config-toml-schema.md | Defines the concrete `config.toml` and `auth.json` schemas resolved by this precedence design. |
| specified-by | TBD | TBD | specs/L3/app/TBD.md | L3 behavior has not been authored yet. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-22 | Assistant | Initial | Initial configuration precedence and onboarding persistence design. |
| 1 | 2026-05-25 | Human | Refinement | Separated durable configuration writes from session/default model selection and added conservative config file write requirements. |
| 1 | 2026-05-25 | Human | Refinement | Linked configuration precedence to the concrete `config.toml` schema design. |
| 1 | 2026-05-25 | Human | Refinement | Moved durable credential material from configuration records into companion `auth.json` files. |

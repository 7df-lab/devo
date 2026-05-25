---
artifact_id: L2-DES-APP-005
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-25
---

# L2-DES-APP-005 - Config TOML And Auth JSON Schema

## Purpose

Define the durable `config.toml` file shape used by user-scoped and project-scoped configuration, and the companion `auth.json` file shape used for credentials.

This design makes model/provider persistence, `/model` selection defaults, MCP server setup, skill discovery roots, and routine application preferences inspectable and mergeable without requiring clients to understand implementation-specific runtime state. It also keeps API keys and other credential material out of `config.toml`.

## Background / Context

`L2-DES-APP-002` defines where configuration files live and how project-scoped configuration takes precedence over user-scoped configuration. This document defines the TOML schema stored at those locations.

The same schema is used for:

- User-scoped configuration:
  - Windows: `C:\Users\username\.devo\config.toml`
  - Windows auth file: `C:\Users\username\.devo\auth.json`
  - macOS and Linux: `~/.devo/config.toml`
  - macOS and Linux auth file: `~/.devo/auth.json`
- Project-scoped configuration:
  - `project_directory/.dev/config.toml`
  - Project auth file: `project_directory/.dev/auth.json`

The TOML schema stores durable preferences and durable setup records. It must not store transient session state, resolved runtime profiles, provider request payloads, routine client projections, API keys, or other secret values.

The JSON auth schema stores credential material for the same source scopes. `config.toml` refers to auth entries by stable credential id.

## Source Requirements

- `L1-REQ-APP-010` requires persistent configuration, user/project file locations, project-over-user precedence, effective configuration inspection, and deterministic persistence targets.
- `L1-REQ-MODEL-001` requires persisted invocable model configuration.
- `L1-REQ-MODEL-002` requires persisted provider configuration and safe credential handling.
- `L1-REQ-MODEL-003` requires onboarding-created configuration to be restorable.
- `L1-REQ-TUI-010` requires onboarding to submit setup results for persistence.
- `L1-REQ-APP-008` requires MCP integrations to be user-configured and status-visible.
- `L1-REQ-APP-009` requires skills to be discoverable from configured sources with visible missing/unavailable states.
- `L1-REQ-APP-012` requires credential and privacy-safe projections.

## Design Requirement

Configuration should use a single TOML schema version with stable, keyed records. Credential material should use a companion `auth.json` schema version with stable credential ids.

Keyed TOML tables are preferred for mergeable records because project-scoped configuration can replace, disable, or add individual records by stable identifier:

- `[providers.<provider_id>]`
- `[model_bindings.<binding_id>]`
- `[mcp.servers.<server_id>]`
- `[skills.roots.<root_id>]`

Arrays may be used only for ordered scalar settings where record-level override is not needed.

The file should be readable and editable by users, but it is still a program-owned schema. The program should validate before persisting changes and should avoid inventing placeholder values when validation fails.

`config.toml` is the durable configuration file. `auth.json` is the durable credential file. Environment variables and external secret stores are not the designed persistence mechanism for API keys or other credentials.

## Top-Level Shape

Every config file should declare the schema version:

```toml
schema_version = 1
```

Top-level sections:

```text
[defaults]
[providers.<provider_id>]
[model_bindings.<binding_id>]
[tools.web_search]
[mcp.servers.<server_id>]
[skills]
[skills.roots.<root_id>]
[workspace.instructions]
[tui]
[telemetry]
[logging]
```

Sections may be absent. Missing sections mean no values are supplied by that source.

Unknown top-level sections should be preserved on write when possible. Unknown keys under known sections should produce diagnostics unless they are under a documented extension namespace such as `[x.<namespace>]`.

## Complete Example

```toml
schema_version = 1

[defaults]
model_binding = "gpt55-openrouter"
reasoning_effort = "medium"
mode = "default"
theme = "system"
permission_policy = "default"

[providers.openrouter]
enabled = true
name = "OpenRouter"
base_url = "https://openrouter.ai/api/v1"
credential = "openrouter_api_key"

[providers.openai]
enabled = true
name = "OpenAI"
base_url = "https://api.openai.com/v1"
credential = "openai_api_key"

[model_bindings.gpt55-openrouter]
enabled = true
model_slug = "openai/gpt-5.5"
provider = "openrouter"
model_name = "openai/gpt-5.5"
invocation_method = "openai_responses"
default_reasoning_effort = "medium"

[model_bindings.deepseek-v4-pro]
enabled = true
model_slug = "deepseek/deepseek-v4-pro"
provider = "openrouter"
model_name = "deepseek/deepseek-v4-pro"
invocation_method = "openai_chat_completions"
default_reasoning_effort = "high"

[tools.web_search]
enabled = true
mode = "provider"
provider_search_binding = "gpt55-openrouter"

[mcp.servers.github]
enabled = true
display_name = "GitHub"
transport = "stdio"
command = "github-mcp-server"
args = ["stdio"]
startup_policy = "lazy"
trust_policy = "user"
allowed_capabilities = ["tools", "resources"]
roots_policy = "workspace"

[mcp.servers.github.env]
GITHUB_TOKEN = { credential = "github_token" }

[skills]
enabled = true
model_catalog_enabled = true
auto_activate = false

[skills.roots.user]
enabled = true
path = "~/.devo/skills"
trust_policy = "user"

[skills.roots.interop]
enabled = true
path = "~/.agents/skills"
trust_policy = "user"

[workspace.instructions]
fallback_filenames = ["CLAUDE.md", "PROMPT.md"]
max_bytes = 200000

[tui]
theme = "system"
vim_mode = false

[telemetry]
enabled = false

[logging]
level = "info"
```

Companion user-scoped `auth.json` example:

```json
{
  "schema_version": 1,
  "credentials": {
    "openrouter_api_key": {
      "kind": "api_key",
      "value": "sk-or-example",
      "created_at": "2026-05-25T00:00:00Z",
      "updated_at": "2026-05-25T00:00:00Z"
    },
    "openai_api_key": {
      "kind": "api_key",
      "value": "sk-example",
      "created_at": "2026-05-25T00:00:00Z",
      "updated_at": "2026-05-25T00:00:00Z"
    },
    "github_token": {
      "kind": "bearer_token",
      "value": "ghp_example",
      "created_at": "2026-05-25T00:00:00Z",
      "updated_at": "2026-05-25T00:00:00Z"
    }
  }
}
```

## Defaults

`[defaults]` stores durable default selections and user preferences.

Fields:

- `model_binding`: optional binding id from `[model_bindings]`.
- `reasoning_effort`: optional logical reasoning effort string for the default model selection.
- `mode`: optional default interaction mode id.
- `theme`: optional UI theme id.
- `permission_policy`: optional default permission posture.

Rules:

- `model_binding` must reference an enabled effective model binding.
- `reasoning_effort` must be allowed by the selected binding's supported model definition.
- Defaults are not active-session state. After the first user message, changing the active session model or reasoning effort does not rewrite `[defaults]` unless a workflow explicitly persists a default according to `L2-DES-APP-002`.
- If both a binding-level `default_reasoning_effort` and `[defaults].reasoning_effort` are present, `[defaults].reasoning_effort` is the default session selection for `[defaults].model_binding`. The binding-level value remains the default shown when that binding is selected outside the current default.

## Permission Policy

`[defaults].permission_policy` controls the default tool permission posture.

Allowed values:

- `default`: baseline permission behavior. Read-only inspection may proceed where otherwise allowed; mutating file operations, command execution, network access, external side effects, and privileged operations remain subject to normal validation, permission checks, sandbox checks, and user approval when required.
- `auto_review`: review-oriented permission behavior. The runtime should classify tool calls before execution and prefer automatic review/diagnostic feedback for risky or ambiguous operations. User approval is still required when the review cannot prove the action is within policy.
- `full_access`: broad permission behavior for trusted contexts. The runtime minimizes approval prompts for allowed tool calls, but it must still enforce validation, mode constraints, privacy rules, audit recording, and the configured sandbox.

Rules:

- The field name is `permission_policy`.
- `approve_policy` and `approval_policy` are not schema fields. If encountered, they should produce a migration or validation diagnostic rather than silently changing behavior.
- Permission policy does not define filesystem or network isolation. Sandbox policy is a separate execution restriction layer.
- `full_access` does not mean unrestricted host execution when sandbox restrictions are configured.

## Sandbox Direction

The durable sandbox schema is not finalized in this revision. The earlier placeholder value `sandbox = "workspace-write"` is intentionally not part of the current `config.toml` schema.

The sandbox design target is to restrict the system calls available to tool execution processes, especially calls that open, read, write, create, delete, rename, or otherwise mutate filesystem objects.

The sandbox should eventually control:

- Directory read access.
- Directory write access.
- File creation, mutation, rename, and deletion.
- Process execution boundaries where supported.
- Network access, controlled at the domain level.

Conceptual future policy dimensions:

- Filesystem read roots and denied roots.
- Filesystem write roots and denied roots.
- Whether tool execution may spawn child processes.
- Whether network access is disabled, unrestricted, or restricted to allowed domains.
- Domain allowlists and denylists for network-capable tools and processes.

Sandbox policy is enforced by the host around tool execution. It is not a model-visible promise and not a substitute for permission policy, user approval, or tool validation.

## Providers

`[providers.<provider_id>]` stores reusable user-defined provider endpoints.

Required fields for an enabled provider:

- `enabled`: boolean.
- `name`: user-facing provider display name.
- `base_url`: provider API base URL.
- `credential`: credential id from effective `auth.json`.

Optional fields:

- `availability_status`: last known safe status such as `unknown`, `valid`, `auth_required`, or `unavailable`.
- `timeout_ms`: provider request timeout.
- `connect_timeout_ms`: provider connection timeout.

Provider ids are stable program-generated identifiers. Changing `name` must not change the provider id.

Provider records must not contain model-specific fields such as `model_name`, `model_slug`, `invocation_method`, or reasoning effort.

## Credentials

`config.toml` stores credential references. `auth.json` stores credential material.

Provider records reference credentials by id:

```toml
credential = "openrouter_api_key"
```

MCP process environment entries and HTTP auth entries also reference `auth.json` credentials by id:

```toml
[mcp.servers.github.env]
GITHUB_TOKEN = { credential = "github_token" }

[mcp.servers.linear]
auth = { credential = "linear_api_key", scheme = "bearer" }
```

`auth.json` has the following shape:

```json
{
  "schema_version": 1,
  "credentials": {
    "openrouter_api_key": {
      "kind": "api_key",
      "value": "sk-or-example"
    }
  }
}
```

Rules:

- `auth.json` is the only designed durable storage location for API keys and other credential material.
- Environment variables, OS keychains, external credential stores, and inline TOML secrets are not part of this design.
- Routine client projections must show credential status, not plaintext credential values.
- `config.toml` writes must not insert plaintext credential values.
- `auth.json` writes must update only the intended credential entries.
- Errors may name the provider, MCP server, and credential id, but must not print credential values by default.
- Project-scoped `auth.json` is valid only as an explicit user choice because the file may be shared with the project directory.
- When creating project-scoped `auth.json`, the program should make the persistence target visible and should recommend or apply project-local ignore behavior where supported by later implementation design.

Auth records:

- `kind`: `api_key`, `bearer_token`, or another program-known credential kind.
- `value`: secret value.
- `created_at`: optional timestamp.
- `updated_at`: optional timestamp.
- `description`: optional user-facing label.

Credential ids are stable keys inside `auth.json`. Renaming a provider does not rename its credential id.

## Model Bindings

`[model_bindings.<binding_id>]` stores configured invocable models.

Required fields for an enabled binding:

- `enabled`: boolean.
- `model_slug`: canonical supported model slug.
- `provider`: provider id from `[providers]`.
- `model_name`: provider-specific model name used for API requests.
- `invocation_method`: program-known invocation method id.

Optional fields:

- `display_name`: client display override.
- `default_reasoning_effort`: logical reasoning effort selected by onboarding or default setup.
- `availability_status`: last known safe status.

Allowed `invocation_method` values for the initial schema:

- `openai_responses`
- `openai_chat_completions`
- `anthropic_messages`

Rules:

- `model_slug` must exist in the built-in supported model catalog.
- `provider` must reference an enabled effective provider.
- `invocation_method` must be supported by the program and valid for the provider/model combination.
- `default_reasoning_effort` must be absent when the supported model does not allow reasoning.
- `default_reasoning_effort` must be one of the supported model's logical effort values when present.
- The `/model` command's first list is populated from enabled effective model bindings. It may show the binding's model and provider together, but it must ask for reasoning effort as a separate step when the model supports reasoning.

## MCP Servers

`[mcp.servers.<server_id>]` stores configured MCP server connections.

Common fields:

- `enabled`: boolean.
- `display_name`: user-facing server name.
- `transport`: `stdio` or `http`.
- `startup_policy`: `eager`, `lazy`, or `manual`.
- `trust_policy`: `user`, `project`, or `untrusted`.
- `allowed_capabilities`: optional list containing `tools`, `resources`, `resource_templates`, `prompts`, `sampling`, or `elicitation`.
- `roots_policy`: `none`, `workspace`, or `configured`.

Stdio fields:

- `command`
- `args`
- `cwd`
- `[mcp.servers.<server_id>.env]`

HTTP fields:

- `base_url`
- `auth`

Example HTTP credential reference:

```toml
[mcp.servers.linear]
enabled = true
display_name = "Linear"
transport = "http"
base_url = "https://mcp.linear.app"
auth = { credential = "linear_api_key", scheme = "bearer" }
startup_policy = "lazy"
trust_policy = "user"
allowed_capabilities = ["tools", "resources"]
roots_policy = "none"
```

Rules:

- Enabled stdio servers require `command`.
- Enabled HTTP servers require `base_url`.
- Secret-bearing process environment variables and HTTP auth values must reference `auth.json` credential ids.
- The runtime may inject an `auth.json` credential into a child process environment only for the configured server operation that requires it. That runtime injection does not make OS environment variables a credential persistence mechanism.
- Project-scoped MCP servers must be visible to the user before first use because they may start local processes or send workspace data to external services.

## Skills

`[skills]` controls global skill behavior for the configuration source.

Fields:

- `enabled`: whether skill discovery is enabled.
- `model_catalog_enabled`: whether a concise skill catalog may be offered to the model.
- `auto_activate`: whether model-selected skill activation is allowed without an explicit user naming a skill.

`[skills.roots.<root_id>]` stores skill discovery roots.

Fields:

- `enabled`: boolean.
- `path`: directory path.
- `trust_policy`: `user`, `workspace`, `plugin`, or `untrusted`.
- `max_depth`: optional scan depth for package discovery.

Rules:

- Skill roots are discovery roots, not instructions by themselves.
- Supporting files under a skill root are not loaded during configuration load.
- Workspace skill roots should be trust-visible before automatic activation.
- Duplicate skill names must be resolved deterministically or reported as conflicts by the skill catalog, not silently overwritten by configuration merge.

## Tools

Tool configuration should be grouped by tool family.

Initial web search shape:

```toml
[tools.web_search]
enabled = true
mode = "provider"
provider_search_binding = "gpt55-openrouter"
```

Rules:

- `mode = "disabled"` means web search should be unavailable with a clear disabled-state message.
- `mode = "provider"` uses a cloud/provider-backed search path where the selected provider binding supports it.
- `mode = "local"` uses `local_provider` or later local search configuration.
- If the configured search path is invalid or unavailable, the runtime must report the configuration gap rather than fabricating results.

## Workspace Instructions

`[workspace.instructions]` stores project-instruction discovery preferences.

Fields:

- `fallback_filenames`: ordered list of additional instruction filenames after native priority files.
- `max_bytes`: maximum assembled instruction bytes before truncation.

Rules:

- The native instruction priority remains owned by `L2-DES-WORKSPACE-001`.
- Additional filenames are configuration-driven compatibility fallbacks.
- Truncation must be visible to the user or diagnostic projection.

## Record Merge Semantics

The effective configuration is built from user config, then project config.

For scalar settings:

- Project value replaces user value.
- Missing project value leaves user value in effect.

For keyed record collections:

- Record identity is the TOML table key.
- A project record with the same key replaces the entire user record.
- A project record may disable a user record by setting only `enabled = false`.
- Other enabled replacement records must include all required fields for that record type.
- Project records with new keys are added to the effective set.
- User records with keys not mentioned by project config remain in the effective set.

This avoids mixing a project provider endpoint with a user invocation method or model name by accident.

Credential references resolve against effective auth data:

- User `auth.json` provides the base credential set.
- Project `auth.json` overlays user `auth.json` by credential id.
- A project-scoped configuration record that references a user-scoped credential id is allowed only after the project-scoped configuration is visible to the user.
- Invalid or missing credential references produce actionable errors and must not silently fall back to another credential id.

## Validation

The program should validate configuration in two phases:

1. Parse and schema validation for each source independently.
2. Effective configuration validation after precedence is applied.

Source validation catches malformed TOML, unsupported schema versions, wrong value types, missing required fields in enabled records, and unknown keys that cannot be preserved safely.

Auth validation catches malformed JSON, unsupported auth schema versions, wrong value types, missing credential values, duplicate credential ids after source precedence, and credential kinds unsupported by the referring config field.

Effective validation catches references to missing providers, disabled providers, missing model bindings, missing credentials, invalid supported model slugs, invalid invocation methods, unsupported reasoning efforts, invalid MCP transport combinations, and unavailable skill roots.

Invalid higher-priority configuration must produce an actionable error instead of silently falling back to lower-priority values for the same setting.

## Write Safety

Configuration writes should be schema-aware:

- Preserve unrelated sections and unknown extension sections where possible.
- Update only the target section or record.
- Never write placeholder model names, provider ids, invocation methods, or reasoning values after validation failure.
- Validate the full resulting file before replacing the previous file contents.
- Use an atomic write strategy in L3/implementation design.
- Report the target path and setting being changed without exposing plaintext credentials by default.

Auth writes should be schema-aware:

- Update `auth.json`, not `config.toml`, when only a credential value changes.
- Never log or show the prior or new credential value by default.
- Validate the resulting `auth.json` before replacing the previous file contents.
- Use an atomic write strategy in L3/implementation design.

Workflows that create or modify providers, model bindings, MCP servers, skill roots, or credentials are persistence writes. Selecting an already-configured model binding for a running session is not a provider, binding, or credential rewrite.

When a setup flow writes both `config.toml` and `auth.json`, the program should avoid committing a final `config.toml` that references a credential id absent from the final `auth.json`. Exact two-file commit and recovery mechanics belong in L3 design.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-APP-010 | 1 | specs/L1/L1-REQ-APP-010-configuration.md | Defines the concrete `config.toml` and `auth.json` schemas for persistent configuration. |
| related-to | L1-REQ-MODEL-001 | 1 | specs/L1/L1-REQ-MODEL-001-config.md | Model bindings and defaults are persisted in `config.toml`, while credentials are referenced through `auth.json`. |
| related-to | L1-REQ-MODEL-002 | 1 | specs/L1/L1-REQ-MODEL-002-provider.md | User-defined providers are persisted in `config.toml`, with credential material in `auth.json`. |
| related-to | L1-REQ-MODEL-003 | 1 | specs/L1/L1-REQ-MODEL-003-onboard.md | Onboarding writes provider and model binding records into `config.toml` and credentials into `auth.json`. |
| related-to | L1-REQ-TUI-010 | 1 | specs/L1/L1-REQ-TUI-010-onboarding-ui.md | TUI onboarding collects values persisted by this schema. |
| related-to | L1-REQ-APP-008 | 1 | specs/L1/L1-REQ-APP-008-mcp.md | MCP servers are configured through `config.toml` and use `auth.json` credential references. |
| related-to | L1-REQ-APP-009 | 1 | specs/L1/L1-REQ-APP-009-skills.md | Skill roots and enablement are configured through `config.toml`. |
| related-to | L1-REQ-APP-012 | 1 | specs/L1/L1-REQ-APP-012-privacy-data-ownership.md | `auth.json` credential storage and redaction behavior protect secrets. |
| related-to | L2-DES-APP-002 | 1 | specs/L2/app/L2-DES-APP-002-configuration-precedence.md | Source precedence resolves this schema across user and project files. |
| related-to | L2-DES-MODEL-001 | 1 | specs/L2/model/L2-DES-MODEL-001-model-provider-binding.md | Provides concrete persistence fields for providers and model bindings. |
| related-to | L2-DES-MCP-001 | 1 | specs/L2/mcp/L2-DES-MCP-001-mcp-integration-architecture.md | Provides concrete persistence fields for MCP servers. |
| related-to | L2-DES-SKILLS-001 | 1 | specs/L2/skills/L2-DES-SKILLS-001-agent-skills-architecture.md | Provides concrete persistence fields for skill roots and enablement. |
| specified-by | TBD | TBD | specs/L3/app/TBD.md | L3 behavior has not been authored yet. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-25 | Assistant | Initial | Initial TOML schema design for durable user and project configuration. |
| 1 | 2026-05-25 | Human | Refinement | Moved API keys and other credential material into dedicated `auth.json` files and removed environment variables or external stores as the recommended credential persistence mechanism. |

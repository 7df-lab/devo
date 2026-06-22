---
artifact_id: L1-REQ-MODEL-001
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Human
last_updated: 2026-05-26
---

# L1-REQ-MODEL-001 — Model Configuration

## Purpose

Let users configure supported models for use by the program.

## Why This Matters

Model behavior depends on capabilities such as context length, reasoning support, supported modalities, and provider availability. Users need clear model configuration to choose a model that fits the task.

## Background / Context

Models differ in context length, reasoning support, reasoning support, supported input modalities, and availability. The program includes a built-in supported-model list that defines model capabilities and default behavior. This built-in list is distinct from user-defined providers, user-provided provider details such as base URL and API key, and model-provider binding details such as provider-specific model name, configurable model display name, and invocation method.

Initially, supported model definitions may exist even when no models have been configured for actual invocation. A model becomes invocable only after the user binds a supported model to a user-defined provider, provider-specific model name, configurable model display name, invocation method, and required reasoning settings where applicable.

Client interfaces may collect credential material during explicit model or provider configuration flows. Routine model selection, model listing, and model-switching views should represent credential state without requiring plaintext credential values.

After onboarding, the primary client-side model change workflow is the TUI `/model` command. This workflow lets the user select a configured or configurable supported model and then choose a reasoning effort when the selected model supports reasoning.

User-configured providers and model-provider bindings are durable configuration records. Their effective values follow the application configuration precedence rule: project-scoped configuration takes precedence over user-scoped configuration for overlapping settings.

## User / Business Requirement

The program must support built-in supported model definitions and user-configured invocable models derived from those supported definitions.

## Real User Scenarios

- A user chooses a model from the built-in supported-model list, selects or creates a provider, enters the provider-specific model name, accepts or edits the model display name, and expects that binding to become available for invocation.
- A user enters an API key during an explicit setup flow and later expects the model switcher to show that credentials are configured without showing the plaintext API key.
- A user invokes `/model` in the TUI, selects a model, chooses a supported reasoning effort, and expects later turns in the same session to keep using that selection.
- A user chooses a reasoning-capable model for a complex task and expects the interface to show that capability.
- A user opens the model switcher and expects to see models that have been configured for actual use.

## Functional Requirements

- The program must include a built-in supported-model list.
- The built-in supported-model list must be defined by a single comprehensive configuration source, such as a JSON file.
- Built-in supported model definitions must include intrinsic model information such as base instructions, context window length, effective context window length, reasoning capabilities, supported modalities, and a default human-readable model display name.
- Built-in supported model definitions must not include user-specific provider invocation details such as provider name, base URL, API key, provider-specific model name, user-configured display name, or invocation method.
- The program must distinguish supported model definitions from user-configured invocable models.
- The program must distinguish reusable user-defined providers from model-provider bindings.
- A user-defined provider must represent a reusable provider connection endpoint and credentials.
- A model-provider binding must represent an invocable model by linking a supported model, a user-defined provider, a provider-specific model name, a configurable model display name, an invocation method, and reasoning effort where applicable.
- The configurable model display name must be used only for user-facing display and must not be treated as the stable model identifier.
- When no custom model display name is entered for a binding, the program must derive one from the supported model definition and persist it with the binding before showing the model in client interfaces.
- Initially, no model is configured for actual invocation unless user configuration has been completed or restored from persistence.
- The user must be able to configure a model for invocation only when that model exists in the built-in supported-model list.
- When configuring a supported model for invocation, the user must select or create a provider, enter the model name expected by that provider, accept or edit a model display name for UI display, and choose an invocation method where applicable.
- Client interfaces may accept credential material when the user is explicitly creating, updating, or repairing model provider configuration.
- Routine client-side model listing, model selection, and model-switching data must expose credential or configuration status instead of plaintext credential values.
- A successfully configured model must become available for selection in client-side model switching interfaces such as the TUI model switcher.
- User-configured providers and model-provider bindings created during onboarding or model setup must be persistently saved to configuration.
- When both project-scoped and user-scoped configuration files define overlapping model provider or model-provider binding settings, the project-scoped configuration must take precedence.
- After onboarding, the TUI must provide a `/model` command for changing the current session model selection.
- The `/model` workflow must first let the user select a model.
- When the selected model supports reasoning, the `/model` workflow must let the user select a supported reasoning effort after model selection.
- A model and reasoning effort selected through `/model` during an active session must become the current session selection and continue to apply to later turns in that session.
- Model configuration must capture user-relevant capabilities such as context length, reasoning support, and supported modalities.
- Model configuration must represent whether a model accepts text, image, and video input where applicable.
- Model configuration must support persistence and onboarding.

## Non-Functional Requirements

- Invalid model configuration must produce actionable errors.
- Model configuration must be understandable to users selecting a model.

## Acceptance Criteria

- Given a model exists in the built-in supported-model list, when the user configures required provider details for it, then the model becomes invocable.
- Given a provider has already been configured, when the user configures another supported model through that provider, then the user can reuse that provider without re-entering base URL and API key.
- Given a supported model is exposed through a provider under a provider-specific model name, when the user configures the binding, then the user can enter that model name separately from the canonical supported model slug.
- Given a user configures an invocable model binding, when the binding is displayed in a client interface, then the client shows the configured model display name and keeps the canonical supported model slug as the stable identifier.
- Given no custom display name is entered for an invocable model binding, when the binding is persisted, then the program stores the supported model definition's default display name as the binding display name.
- Given the user enters credential material in an explicit model configuration flow, when the configuration is submitted, then the program can use that credential material to configure the model for invocation.
- Given a model does not exist in the built-in supported-model list, when the user attempts to configure it for invocation, then the program rejects the configuration with an actionable explanation.
- Given no user model configuration exists, when the user opens model selection, then the program does not present unconfigured supported models as ready for invocation.
- Given a configured model, when the user opens a client-side model switching interface, then that model is available for selection.
- Given a configured model appears in a routine client-side model switching interface, when the client displays it, then credential state is represented as status rather than as a plaintext credential value.
- Given onboarding or model setup creates a provider and model-provider binding, when the program restarts, then that invocable model remains available from persistent configuration.
- Given project-scoped and user-scoped configuration define overlapping model defaults or bindings, when the program computes available invocable models, then the project-scoped configuration takes precedence for those overlapping settings.
- Given the user invokes `/model` after onboarding, when the model selection opens, then the user can select a model before selecting reasoning effort.
- Given the selected model supports reasoning, when the user selects it through `/model`, then the user can choose one of that model's supported reasoning efforts.
- Given the user changes model or reasoning effort through `/model` during a session, when the next turn starts in that session, then the changed selection remains active.
- Given a first-time user, when onboarding requires model setup, then the user can configure or select a supported model.
- Given a model lacks a capability required by a task, when the user selects it, then the program reports the limitation before relying on that capability.
- Given a model has configured modality capabilities, when the program prepares a request, then those capabilities can be used to decide which context modalities are allowed.
- Given model configuration is persisted, when the program restarts, then configured models remain available.

## Out of Scope

- The program does not define exact model catalog schema, file path, provider request format, or client UI layout in this L1 requirement.
- The program does not define exact credential-entry controls, credential-reveal controls, or credential-store backend in this L1 requirement.
- This requirement does not define exact popup layout, search behavior, keyboard handling, or visual styling for the `/model` workflow.
- This requirement does not guarantee that every configured model supports every program feature.
- This requirement does not allow arbitrary models outside the built-in supported-model list to become invocable without first being added to that supported-model list.

## Open Questions

- Which model capabilities are mandatory for the initial configuration UI?
- Which modality capability fields are required for built-in supported model definitions?
- Who is allowed to update the built-in supported-model list, and through what review process?
- Should users be able to request support for a new model from the client interface?

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refined-by | L2-DES-MODEL-001 | 1 | specs/L2/model/L2-DES-MODEL-001-model-provider-binding.md | L2 defines supported models, user providers, and model-provider bindings. |
| related-to | L2-DES-APP-002 | 1 | specs/L2/app/L2-DES-APP-002-configuration-precedence.md | L2 defines configuration source precedence used by persisted model provider and binding records. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-20 | Assistant | Initial | Initial draft with approved L1 refinement. |
| 1 | 2026-05-21 | Human | Refinement | Added explicit text, image, and video modality capability requirements. |
| 1 | 2026-05-21 | Human | Refinement | Distinguished built-in supported model definitions from user-configured invocable models. |
| 1 | 2026-05-22 | Human | Refinement | Clarified that explicit configuration flows may accept credentials while routine model selection data exposes credential status rather than plaintext credentials. |
| 1 | 2026-05-22 | Human | Refinement | Added `/model` as the post-onboarding TUI workflow for changing the current session model and supported reasoning effort. |
| 1 | 2026-05-22 | Human | Refinement | Split user-defined providers from model-provider bindings, moved invocation method to the binding, and removed tool support from supported model metadata. |
| 1 | 2026-05-22 | Human | Refinement | Added persistent storage and project-over-user precedence for configured providers and model-provider bindings. |
| 1 | 2026-05-26 | Human | Refinement | Added configurable model display name as binding display metadata distinct from the canonical model slug and provider-specific model name. |

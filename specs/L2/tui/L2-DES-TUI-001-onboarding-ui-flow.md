---
artifact_id: L2-DES-TUI-001
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-25
---

# L2-DES-TUI-001 — Onboarding UI Flow

## Purpose

Refine the TUI onboarding requirement into a concrete terminal interaction design for required model setup.

## Background / Context

The TUI onboarding flow must let a user configure an invocable model without leaving the terminal. L1 requires model slug selection, provider selection or creation, provider-specific model name entry, invocation method selection, and reasoning effort selection where supported.

This L2 design defines the concrete terminal flow, inline presentation, popup behavior, and interaction sequence. It does not define storage backends, provider validation protocol, or final visual styling values.

## Source Requirements

- `L1-REQ-TUI-010` requires TUI onboarding for required model setup.
- `L1-REQ-MODEL-001` defines supported model definitions, invocable model configuration, and credential status expectations.
- `L1-REQ-MODEL-002` defines provider setup and provider availability behavior.
- `L1-REQ-MODEL-003` defines onboarding as the product-level setup path.
- `L1-REQ-APP-010` defines persistent configuration and project-over-user configuration precedence.
- `L1-REQ-APP-012` defines privacy and credential-handling expectations.
- `L2-DES-MODEL-001` defines the supported model, user provider, and model-provider binding records created by this flow.
- `L2-DES-APP-002` defines the configuration source precedence and default persistence target behavior.

## Design Requirement

The TUI onboarding UI should use a searchable popup for discrete selections and an inline form for provider, model name, and credential entry. Every active input field or popup must show a concise hint that explains the current value expected from the user.

The flow order is:

1. Search and select supported model slug.
2. Close the model slug popup after confirmation.
3. Select an existing provider or choose to add a provider.
4. If adding a provider, enter provider name.
5. If adding a provider, enter base URL.
6. If adding a provider, enter API key.
7. Enter the model name expected by the selected provider.
8. Search and select invocation method.
9. Search and select reasoning effort when the selected model supports reasoning.
10. Persist setup results to configuration.
11. Complete setup and continue to a usable session.

## Interaction Sketch

The following ASCII sketch defines the required interaction structure and visible control groups. It is not a final styling specification for dimensions, color, or focus rings.

Onboarding controls should be visually unframed. Popup sections must not use outer ASCII box borders such as `+--------+` or full-frame side borders. The inline setup stack should use a single vertical rail to connect the configured fields from top to bottom.

```text
Select Model Slug
Hint: Choose the model capability profile the program should use.

Search: gpt

> openai/gpt-5.5
  openai/gpt-5.4
  anthropic/claude-opus
  local/qwen3-coder

Enter: select and close popup
Esc: cancel

Select Provider
Hint: Choose a provider or add one.

Search: open

> OpenAI
  OpenRouter
  Add provider...

Enter: select and close popup
Esc: back

Model: openai/gpt-5.5
|
* provider name:
| Hint: Enter a name to recognize this provider later.
| OpenRouter
|
* base url:
| Hint: Enter the provider API base URL.
| https://api.example
|
* api key:
| Hint: Enter the API key for this provider.
| [hidden input]
|
* model name:
| Hint: Enter the model name this provider expects.
| openai/gpt-5.5
|
* invocation method:
| Hint: Choose the API protocol used for this binding.
| [open popup]
|
* reasoning effort:
| Hint: Choose the default reasoning effort for this binding.
| [open popup if the model supports reasoning]
|

Invocation Method
Hint: Choose the API protocol used to call this model.

Search: openai

> OpenAI Responses
  OpenAI Chat Completions
  Anthropic Messages

Enter: select and close popup
Esc: back

Reasoning Effort
Hint: Choose the default reasoning effort for this binding.

> medium
  high
  xhigh

Enter: select and close popup
Esc: back
```

## Flow Behavior

- The model slug selector is the first control in model onboarding.
- The model slug selector must show a hint that tells the user they are choosing the model capability profile the program should use.
- The model slug selector must support search or filtering by slug text.
- Pressing Enter on a highlighted model slug confirms the selection and closes the popup.
- Pressing Esc from the model slug selector cancels onboarding or returns to the previous onboarding step where one exists.
- After model slug confirmation, the provider selector opens.
- The provider selector must show existing providers plus an add-provider option.
- The provider selector must show a hint that tells the user to choose an existing provider or add one.
- Pressing Enter on a highlighted provider confirms the selection and closes the popup.
- If the user chooses to add a provider, provider detail entry is inline rather than a boxed popup.
- The inline setup view must display the selected model slug before editable provider fields.
- The inline setup view must use a single continuous vertical rail to connect model display, provider name entry, base URL entry, API key entry, model name entry, invocation method selection, and reasoning effort selection where applicable. The rail should appear under each field marker rather than before the `* field` label.
- The inline setup rail is a guide for the setup sequence, not an outer frame; it must not wrap the content on both sides or draw top/bottom box borders.
- Provider name entry must appear before base URL entry when adding a provider.
- Provider name entry must show a hint that tells the user to enter a name for recognizing the provider later.
- Base URL entry must show a hint that tells the user to enter the provider API base URL.
- API key entry must use hidden or masked input by default.
- API key entry must show a hint that tells the user to enter the API key for the selected provider.
- Model name entry appears after provider selection or provider creation.
- Model name entry must show a hint that tells the user to enter the model name this provider expects for API calls.
- Invocation method selection appears after model name entry.
- Invocation method selection uses the same search-popup interaction pattern as model slug selection.
- Invocation method selection must show a hint that tells the user to choose the API protocol used to call this model through this provider.
- Invocation method choices include OpenAI Responses, OpenAI Chat Completions, and Anthropic Messages where available.
- Pressing Enter on a highlighted invocation method confirms the selection, closes the popup, and returns to the inline setup view.
- If the selected model supports reasoning, reasoning effort selection appears after invocation method selection.
- Reasoning effort selection uses the same search-popup interaction pattern as model slug selection.
- Reasoning effort selection must show a hint that tells the user to choose the default reasoning effort for this model binding.
- Pressing Enter on a highlighted reasoning effort confirms the selection, closes the popup, and returns to the inline setup view.
- If the selected model does not support reasoning, the inline setup view omits the reasoning effort selection step.
- Successful setup submits the selected values for persistent configuration storage before normal model invocation begins.
- If onboarding runs with an active project directory and no explicit target selection is available, the default persistence target is the project-scoped configuration file.
- If onboarding runs without an active project directory and no explicit target selection is available, the default persistence target is the user-scoped configuration file.
- Validation failures should preserve the selected model slug and safe completed fields where useful.

## Error And Recovery Behavior

- Invalid base URL input should produce a concise inline error near the base URL field.
- Invalid or rejected API key input should produce a concise provider setup error without writing the plaintext key into transcript history.
- Unsupported invocation method selection should be prevented by the selection list where possible.
- If provider validation fails after submission, the TUI should return to the inline setup view with safe completed fields preserved.
- If persistence fails after valid setup input, the TUI should report the configuration target and return to a recoverable setup state.
- The user should be able to go back from provider, invocation method, and reasoning effort popups without losing earlier safe fields.

## Privacy Constraints

- API key entry is an explicit credential-handling flow.
- Plaintext API keys must not appear in routine transcript, model list, model switcher, logging, or telemetry paths by default.
- The inline setup view may show credential status or masked input, but must not display plaintext credential values by default after entry.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-TUI-010 | 1 | specs/L1/L1-REQ-TUI-010-onboarding-ui.md | Provides the concrete terminal interaction design for TUI onboarding. |
| related-to | L1-REQ-MODEL-001 | 1 | specs/L1/L1-REQ-MODEL-001-config.md | Uses supported model and invocable model configuration requirements. |
| related-to | L1-REQ-MODEL-002 | 1 | specs/L1/L1-REQ-MODEL-002-provider.md | Uses provider setup requirements. |
| related-to | L1-REQ-MODEL-003 | 1 | specs/L1/L1-REQ-MODEL-003-onboard.md | Refines the TUI presentation of model onboarding. |
| related-to | L1-REQ-APP-010 | 1 | specs/L1/L1-REQ-APP-010-configuration.md | Uses persistent configuration and project-over-user precedence requirements. |
| related-to | L1-REQ-APP-012 | 1 | specs/L1/L1-REQ-APP-012-privacy-data-ownership.md | Carries credential-handling constraints into UI design. |
| related-to | L2-DES-MODEL-001 | 1 | specs/L2/model/L2-DES-MODEL-001-model-provider-binding.md | The flow creates user provider and model-provider binding records. |
| related-to | L2-DES-APP-002 | 1 | specs/L2/app/L2-DES-APP-002-configuration-precedence.md | Defines where successful onboarding results are persisted and how they are resolved. |
| specified-by | TBD | TBD | specs/L3/tui/TBD.md | L3 behavior has not been authored yet. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-22 | Assistant | Initial | Initial L2 design extracted from the approved concrete TUI onboarding sketch. |
| 1 | 2026-05-22 | Human | Refinement | Updated the flow to model-first, provider-select-or-add, provider name/base URL/API key, provider-specific model name, invocation method, reasoning effort, and per-field hints. |
| 1 | 2026-05-22 | Human | Refinement | Added persistent configuration storage and default target behavior for successful onboarding. |
| 1 | 2026-05-25 | Human | Refinement | Removed outer ASCII frames while keeping a continuous inline rail under the setup field markers. |

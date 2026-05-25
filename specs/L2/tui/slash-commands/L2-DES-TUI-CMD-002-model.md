---
artifact_id: L2-DES-TUI-CMD-002
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-25
---

# L2-DES-TUI-CMD-002 — Slash Command: /model

## Purpose

Define the TUI behavior for `/model`, the post-onboarding command for changing the active session model, provider binding, invocation method, and reasoning effort where applicable.

## Command Contract

- Command: `/model`
- Description: `choose the active model`
- Parameters: none in the first milestone.
- Mutability: session metadata; model-provider configuration only when adding or repairing configuration; default-selection configuration only where the application configuration lifecycle requires it.
- Active-turn availability: blocked while a turn is generating, running tools, or waiting on active execution.

## Design Requirement

`/model` should first show configured model-provider bindings from effective configuration. These bindings may have been created by onboarding or defined directly in configuration files. The last row in the configured binding list is always `Add model...`.

Selecting an existing configured binding does not immediately apply the final session model when the model supports reasoning. `/model` groups model and provider together as the configured binding step, then treats reasoning effort as a distinct follow-up step. If the selected model does not support reasoning, the configured binding can be applied immediately. If the user selects `Add model...`, `/model` enters the same add-model setup sequence as onboarding:

1. Select configured model-provider binding, or choose `Add model...`.
2. If adding a model, select supported model slug.
3. If adding a model, select an existing provider or add a provider.
4. When adding a provider, enter provider name.
5. When adding a provider, enter base URL.
6. When adding a provider, enter API key.
7. Enter the model name expected by the selected provider.
8. Select invocation method when a new binding requires it.
9. Select reasoning effort when the selected model supports reasoning.
10. Apply the resulting model-provider binding and reasoning effort to future turns in the current session, persisting newly created provider or binding records before treating them as configured.

The interaction surface differs from onboarding. Onboarding may show a longer inline setup stack because it is the initial setup experience. `/model` is a focused slash-command workflow and should show only the current step directly below the composer, using the slash-command popup visual grammar from `L2-DES-TUI-003`. The slash-command surface occupies the same bottom region as the bottom status line, so while `/model` is visible the normal bottom status line is hidden. When the user confirms or submits a step, that step disappears and the next step replaces it in the same below-composer command area. Previously completed steps are retained in command state but are not rendered as visible inline history.

## UI Flow

`/model` opens a transient command surface below the composer. The first surface is a configured model-provider binding list, not the supported-model slug selector. The configured binding list may show model and provider together, but it must not conflate reasoning effort into the binding-selection row. Discrete choices use the same searchable popup pattern as onboarding and the row layout rules from `L2-DES-TUI-003`: two-character left padding, active row primary foreground, inactive command/name text in normal foreground, and secondary details in muted foreground. Free-text values use a single active input prompt below the composer.

```text
┃ /model

> deepseek-v4-pro   OpenRouter
  gpt-5.5           OpenAI
  claude-sonnet-5   Anthropic
  Add model...
```

Selecting an existing configured binding removes the binding list and shows the reasoning effort step when the selected model supports reasoning:

```text
┃ /model

  Reasoning Effort
  Hint: Choose the reasoning effort for gpt-5.5 through OpenAI.

  > medium
    high
    xhigh

  Enter: select and apply
  Esc: back
```

Selecting `Add model...` removes the configured binding list and shows the first add-model step:

```text
┃ /model

  Select Model Slug
  Hint: Choose the model capability profile the session should use.

  Search: gpt

  > openai/gpt-5.5
    openai/gpt-5.4
    anthropic/claude-opus
    local/qwen3-coder

  Enter: select and continue
  Esc: back
```

After selecting a model slug in the add-model flow, the model selector is removed and the provider step appears:

```text
┃ /model

  Select Provider
  Hint: Choose a provider or add one.

  Search: open

  > OpenAI
    OpenRouter
    Add provider...

  Enter: select and continue
  Esc: back
```

When a free-text step is active, only that step is shown:

```text
┃ /model

  Model Name
  Hint: Enter the model name this provider expects.

  openai/gpt-5.5

  Enter: continue
  Esc: back
```

When an invocation method is required for a new binding, the command surface replaces the previous step with the invocation selector:

```text
┃ /model

  Invocation Method
  Hint: Choose the API protocol used to call this model.

  Search: openai

  > OpenAI Responses
    OpenAI Chat Completions
    Anthropic Messages

  Enter: select and continue
  Esc: back
```

The below-composer command surface must not render the onboarding-style inline history stack:

```text
Model: openai/gpt-5.5
|
* provider name:
| ...
|
* base url:
| ...
```

That stacked rail view belongs to onboarding. `/model` uses one active step at a time below the composer.

## Step Behavior

- The configured model-provider binding list is the first `/model` step.
- The configured binding list must be populated from effective configuration, including bindings created by onboarding and bindings defined directly in configuration files.
- The configured binding list must show configured model-provider bindings and an `Add model...` row at the bottom. It may show provider identity in each row, but it must not include reasoning effort.
- While any `/model` command surface is visible, the normal bottom status line must not be rendered because the command surface occupies that same area below the composer.
- Pressing Enter on a highlighted configured binding records the selected model-provider binding in command-local state.
- Reasoning effort selection is required after configured binding selection when the selected model supports reasoning, even when the binding already has a default or last-used reasoning effort.
- If the selected configured binding's model does not support reasoning, the binding may be applied immediately after selection.
- The final selection is applied only after the configured binding and any required reasoning effort step have completed.
- Selecting an existing configured binding after the first user message updates the current session selection only; it must not rewrite provider records, binding records, or default-selection fields.
- Selecting an existing configured binding before the first user message may persist the default selected binding and reasoning effort according to application configuration rules, but it must not duplicate or rewrite unchanged provider and binding records.
- Pressing Enter on `Add model...` removes the configured binding list and starts the add-model flow at supported model slug selection.
- The model slug selector must support search or filtering by slug text.
- Pressing Enter on a highlighted model slug confirms the slug, removes the model selector, and shows the provider step.
- The provider step must let the user choose an existing provider or add a provider.
- If the user chooses to add a provider, `/model` prompts for provider name, base URL, and API key as separate current-step views below the composer.
- API key entry must use hidden or masked input by default.
- After provider selection or creation, `/model` prompts for the provider-specific model name.
- Invocation method selection appears after model name entry and uses the same searchable selection pattern as onboarding.
- If the selected model supports reasoning, reasoning effort selection appears after invocation method selection and uses the same searchable selection pattern as onboarding.
- If the selected model does not support reasoning, the reasoning effort step is skipped.
- Pressing Esc returns to the previous step when one exists; otherwise it cancels `/model` and clears the below-composer command surface.
- Completed step values are stored in command-local state so back navigation and final application remain correct, but completed steps are not shown as inline history below the composer.
- The command may show a concise final confirmation or success status after applying the selection, but it must not expand the full completed-step history.

## State And Error Behavior

- The TUI should use `model.list` to populate configured model-provider bindings for the first `/model` screen and supported model choices for the `Add model...` flow.
- The final selection should use `model.select`.
- New or modified provider and model-provider binding data should follow the same validation and persistence expectations as onboarding and should be persisted before the command applies a newly created binding.
- Persistence errors for newly created or modified configuration must keep `/model` in a recoverable command state rather than silently falling back to a session-only binding.
- The command must show credential status but must not display plaintext API keys in routine lists.
- If invoked during active work, the TUI shows a concise blocked message such as `Cannot change model while generating`.
- The selected model and reasoning effort affect the next turn, not an already-running invocation.
- Validation failures keep the user on the current below-composer step with a concise error near that step.
- If persistence fails after valid setup input, `/model` reports the target configuration scope and leaves the command in a recoverable state.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-TUI-006 | 1 | specs/L1/L1-REQ-TUI-006-command-discovery-control.md | Defines `/model`, the required post-onboarding model-selection command. |
| related-to | L1-REQ-MODEL-001 | 1 | specs/L1/L1-REQ-MODEL-001-config.md | Model selection uses configured model-provider bindings. |
| related-to | L1-REQ-APP-010 | 1 | specs/L1/L1-REQ-APP-010-configuration.md | Defines when model selection changes are persisted as defaults versus session state. |
| related-to | L2-DES-MODEL-001 | 1 | specs/L2/model/L2-DES-MODEL-001-model-provider-binding.md | Defines supported models, user providers, and model-provider bindings. |
| related-to | L2-DES-APP-002 | 1 | specs/L2/app/L2-DES-APP-002-configuration-precedence.md | Defines configuration write scope, persistence target behavior, and distinction between session selection and durable records. |
| related-to | L2-DES-TUI-001 | 1 | specs/L2/tui/L2-DES-TUI-001-onboarding-ui-flow.md | Reuses the onboarding model setup sequence while using a transient below-composer command surface instead of an inline history stack. |
| related-to | L2-DES-TUI-003 | 1 | specs/L2/tui/L2-DES-TUI-003-composer-and-input-modes.md | Uses shared slash-command discovery, popup, and invocation behavior. |
| specified-by | TBD | TBD | specs/L3/tui/TBD.md | L3 behavior has not been authored yet. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-23 | Assistant | Initial | Initial `/model` command design. |
| 1 | 2026-05-25 | Human | Refinement | Aligned `/model` with the onboarding model setup sequence and specified one-step-at-a-time rendering below the composer. |
| 1 | 2026-05-25 | Human | Refinement | Changed the first `/model` screen to configured binding selection with `Add model...` as the entry to the add-model flow. |
| 1 | 2026-05-25 | Human | Refinement | Clarified that the `/model` command surface replaces the bottom status line while visible. |
| 1 | 2026-05-25 | Human | Refinement | Split configured model, provider, and reasoning effort into distinct `/model` steps. |
| 1 | 2026-05-25 | Human | Refinement | Grouped model and provider back into the configured binding selection while keeping reasoning effort separate. |
| 1 | 2026-05-25 | Human | Refinement | Clarified that existing binding selection is session state after the first user message, while newly created provider or binding records require configuration persistence. |

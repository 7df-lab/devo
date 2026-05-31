---
artifact_id: L3-BEH-TUI-005
revision: 3
status: Draft
active_baseline: no
---

# L3-BEH-TUI-005 — Onboarding UI Flow

## Purpose

Define the concrete TUI behavior for the first-run model setup onboarding flow: model slug selection, provider selection/creation, inline form entry, invocation method and reasoning effort selection, validation, error recovery, and persistence.

## Source Design

L2-DES-TUI-001 (Onboarding UI Flow), L2-DES-MODEL-001 (Model Provider Binding), L2-DES-APP-007 (CLI Onboarding Entry)

## Behavior Specification

### B1. Onboarding Entry

- **Trigger**: No valid model bindings exist (checked at CLI startup per L3-BEH-CLI-001 B2), or user runs `devo --onboard`.
- **Preconditions**: Terminal is interactive. TUI backend is initialized.
- **Algorithm / Flow**:
  1. Display a minimal TUI shell (no transcript, no composer — full-screen onboarding).
  2. Show a centered title: "Welcome to Devo — Let's set up your first model."
  3. Begin the 11-step flow starting with model slug selection (B2).
- **Postconditions**: Onboarding flow is active. Normal slash-command handling is disabled during onboarding.

### B2. Step 1 — Model Slug Selection

- **Trigger**: Onboarding entry.
- **Preconditions**: Built-in supported model catalog is loaded.
- **Algorithm / Flow**:
  1. Render a searchable popup with hint: "Choose the model capability profile the program should use."
  2. Show a search/filter input at the top. As user types, filter the model list by slug substring match (case-insensitive).
  3. List matching models. Each row shows `canonical_model_slug`. Currently focused row is marked with `>`.
  4. Navigation: Up/Down arrows move focus. Enter selects the focused model. Esc cancels onboarding.
  5. On selection: store the chosen `canonical_model_slug`. Close popup. Proceed to provider selection (B3).
- **Postconditions**: A supported model slug is selected. The choice is visible in subsequent steps.

### B3. Step 2 — Provider Selection

- **Trigger**: Model slug confirmed.
- **Preconditions**: Existing user providers are loaded from config.
- **Algorithm / Flow**:
  1. Render a searchable popup with hint: "Choose an existing provider or add one."
  2. List existing providers by `provider_name`. Each with `>` focus marker. Last row: `Add provider...`.
  3. If user selects an existing provider: store the `provider_id`. Skip to model name entry (B5).
  4. If user selects `Add provider...`: proceed to inline provider entry (B4).
  5. Esc: return to model slug selection (B2), preserving the selection.
- **Postconditions**: A provider is selected or the add-provider flow begins.

### B4. Steps 3-5 — Inline Provider Entry

- **Trigger**: User chose `Add provider...`.
- **Preconditions**: The model slug is selected.
- **Algorithm / Flow**:
  1. Render inline setup view with a vertical rail connecting fields:
     - Top: `Model: <canonical_model_slug>` (read-only, shows the selection from B2).
     - `* provider name:` — text input with hint "Enter a name to recognize this provider later." Validation: non-empty, max 100 chars.
     - `* base url:` — text input with hint "Enter the provider API base URL." Validation: must parse as `url::Url`, scheme http/https.
     - `* api key:` — masked text input with hint "Enter the API key for this provider." Characters are shown as `*` or hidden. Validation: non-empty.
  2. Fields are navigated with Tab/Shift+Tab or Up/Down. Each field shows its hint below.
  3. On Enter in api key field (last field): validate all fields. If valid, store provider data. Proceed to model name entry (B5).
  4. If validation fails: show inline error near the invalid field. Preserve filled field values.
  5. Esc from any field: return to provider selection (B3). If provider data was partially entered, warn before discarding.
- **Postconditions**: Provider data (name, base_url, api_key) is captured in memory. Not yet persisted.

### B5. Steps 6-7 — Model Name and Display Name

- **Trigger**: Provider is selected or created.
- **Preconditions**: Model slug and provider are known.
- **Algorithm / Flow**:
  1. Extend the inline setup rail with:
     - `* model name:` — text input with hint "Enter the model name this provider expects for API calls." Prefilled with `canonical_model_slug` as a suggestion. Validation: non-empty, max 200 chars.
     - `* display name:` — text input with hint "Enter the name clients should show for this model." Prefilled with the supported model's `display_name`. Validation: non-empty, max 100 chars.
  2. Enter advances to next field. Last field (display name) advances to invocation method (B6).
  3. Values are editable; the user may accept or modify prefilled suggestions.
- **Postconditions**: Model name and display name are captured.

### B6. Steps 8-9 — Invocation Method and Reasoning Effort

- **Trigger**: Display name entered.
- **Preconditions**: Model name is captured.
- **Algorithm / Flow**:
  1. **Invocation method**: render a searchable popup with hint "Choose the API protocol used to call this model through this provider."
     - Options: `OpenAI Responses`, `OpenAI Chat Completions`, `Anthropic Messages`.
     - `>` marks focused row. Enter confirms. Esc returns to display name field.
  2. After invocation method selected:
     a. If the chosen model supports reasoning (`reasoning_capability != Unsupported`): render reasoning effort popup with hint "Choose the default reasoning effort for this model binding."
        - Options derived from model's `reasoning_capability` (e.g., low, medium, high, xhigh, max, adaptive).
        - `>` marks focused row. Enter confirms. Esc returns to invocation method selection.
     b. If model does not support reasoning: skip this step. Proceed directly to persistence (B7).
- **Postconditions**: All required fields are captured. Setup is ready for persistence.

### B7. Persistence and Completion

- **Trigger**: All fields are captured and validated.
- **Preconditions**: All required values are present.
- **Algorithm / Flow**:
  1. Determine persistence target:
     - If a workspace is active → persist non-secret configuration to workspace config (`<workspace>/.devo/config.toml`) and persist credential material to user-scoped `auth.json`.
     - If no workspace is active → persist non-secret configuration to the user-scoped config (`~/.devo/config.toml` on macOS/Linux, `C:\Users\username\.devo\config.toml` on Windows) and persist credential material to user-scoped `auth.json`.
  2. Create `UserProvider` record (if new provider was added): generate `provider_id`, store `provider_name`, `base_url`, `credential_ref`.
  3. Store API key in `auth.json` under the generated credential id.
  4. Create `ModelProviderBinding`: generate `binding_id`, store `canonical_model_slug`, `provider_id`, `model_name`, `display_name`, `invocation_method`, `reasoning_effort`, `is_default: true`.
  5. Write config files atomically (write to temp file, rename).
  6. If any write fails: show error with the config target path. Allow retry or return to setup to modify values.
  7. On success: show "Setup complete!" message for 2 seconds. Exit onboarding and proceed to normal session start (connect server, launch TUI).
- **Postconditions**: At least one valid model binding exists. Config is persisted. Session can start.

### B8. Error Recovery

- **Trigger**: Validation or persistence failure at any step.
- **Preconditions**: Some fields may be valid.
- **Algorithm / Flow**:
  1. **Invalid base URL**: show inline error "Invalid URL. Enter a valid HTTP or HTTPS URL (e.g., https://api.openai.com)." Keep the field value for editing.
  2. **Invalid/empty API key**: show inline error "API key cannot be empty." Do NOT display the key value in the error.
  3. **Persistence failure**: show error with target path and reason (e.g., "Cannot write to ~/.devo/config.toml: permission denied"). Offer retry or return to setup.
  4. **Esc from popups** (provider, invocation method, reasoning effort): return to the previous step preserving all safe completed fields.
  5. **Esc from inline entry** (when adding provider): warn "Discard entered provider details?" before returning to provider selection.
  6. Validated fields are preserved across back-navigation and retry.
- **Postconditions**: The user can recover without losing completed work. No plaintext secrets are written to logs or transcript.

### B9. Privacy Constraints

- **Trigger**: API key entry, display rendering, error logging.
- **Preconditions**: Privacy requirements from L1-REQ-APP-012 apply.
- **Algorithm / Flow**:
  1. API key input field: characters are displayed as `*` or hidden entirely. A toggle keybinding (Ctrl+R) may reveal the input temporarily.
  2. After entry and persistence: the plaintext key must not appear in routine transcript, model list, model switcher, logging, or telemetry.
  3. Inline setup view after key entry: show `credential_status: configured` instead of the key value.
  4. Error messages referencing the API key: say "API key invalid" — never include the key text.
- **Postconditions**: Credential safety is maintained throughout onboarding.

## Traceability

| L2 Source | Relationship |
|---|---|
| L2-DES-TUI-001 | specified-by |
| L2-DES-MODEL-001 | specified-by |
| L2-DES-APP-007 | specified-by |

## Implementation Notes

- Onboarding uses the same TUI infrastructure (Ratatui, crossterm) as the main TUI but in a dedicated "onboarding mode" with no transcript or composer.
- Popup search uses the same fuzzy matching as `@` file search (nucleo) for the model slug filter.
- API key storage: write only to user-scoped `auth.json` with file mode `0600`. Use atomic write (write to `.auth.json.tmp`, `rename` to `auth.json`). The onboarding UI must never write `<workspace>/.devo/auth.json`.
- Persistence target precedence: CLI `--config` flag > active workspace > user config dir.

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-27 | Assistant | Initial | Initial onboarding UI flow. |
| 2 | 2026-05-27 | Assistant | Correction | Replaced stale `devo onboard` and `.config/devo` config paths with the L2-defined `--onboard` and `~/.devo` paths. |
| 3 | 2026-05-27 | Assistant | Correction | Aligned onboarding persistence with user-only `auth.json` and workspace `<workspace>/.devo/config.toml`. |

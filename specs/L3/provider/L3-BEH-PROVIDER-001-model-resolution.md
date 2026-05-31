---
artifact_id: L3-BEH-PROVIDER-001
revision: 3
status: Draft
active_baseline: no
---

# L3-BEH-PROVIDER-001 — Model Resolution and Provider Binding

## Purpose

Define the concrete behavior for resolving supported model definitions, user-defined providers, and model-provider bindings into a runtime `ResolvedModelProfile`, handling provider authentication, and constructing provider-specific requests.

## Source Design

L2-DES-MODEL-001 (Model Provider Binding), L2-DES-APP-005 (Config TOML Schema), L3-BEH-APP-001 (Configuration Resolution And Persistence)

## Behavior Specification

### B1. SupportedModelDefinition Catalog

- **Trigger**: Server startup.
- **Preconditions**: Built-in model definitions are compiled into the binary or loaded from a packaged data file.
- **Algorithm / Flow**:
  1. Load the built-in model catalog. Each entry is a `SupportedModelDefinition`:
     - `canonical_model_slug` (e.g., `openai/gpt-5.5`, `anthropic/claude-opus-4-7`).
     - `display_name` (e.g., "GPT-5.5", "Claude Opus 4.7").
     - `base_instructions`: model-specific base instruction text.
     - `context_window`: total context window in tokens.
     - `effective_context_window`: program-safe effective window (default: `context_window * 0.9`).
     - `modalities`: bitflags or list of supported input modalities.
     - `reasoning_capability`: `Unsupported`, `Binary`, or `NamedReasoning` (vec of allowed efforts: low, medium, high, xhigh, max, adaptive).
     - `default_reasoning_effort`: optional default.
  2. Index by `canonical_model_slug` for O(1) lookup.
  3. The catalog must NOT contain provider names, URLs, API keys, or invocation methods.
- **Postconditions**: All supported models are known to the program.

### B2. UserProvider Validation

- **Trigger**: User configures a provider (via onboarding or config edit).
- **Preconditions**: Effective provider config has been loaded and validated by `L3-BEH-APP-001`.
- **Algorithm / Flow**:
  1. Each `UserProvider` is validated:
     - `provider_id`: generated UUID v4, stable.
     - `provider_name`: user-entered display name (non-empty, max 100 chars).
     - `base_url`: valid URL (must parse as `url::Url`, scheme http or https).
     - `credential_ref`: references a credential id in `auth.json`.
  2. Resolve credential status against the user auth store from `L3-BEH-APP-001`. If not found -> `availability_status: needs_configuration`.
  3. If optional provider validation is configured: send a minimal API request to verify connectivity and credential validity. Set `availability_status: available` or `unavailable`.
  4. Provider must NOT contain: canonical model slug, model name, invocation method, reasoning effort.
- **Postconditions**: Valid providers are available for binding.

### B3. ModelProviderBinding Resolution

- **Trigger**: Server resolves the invocable model list.
- **Preconditions**: Supported models and providers are loaded.
- **Algorithm / Flow**:
  1. Each binding is validated:
     - `binding_id`: stable, generated.
     - `canonical_model_slug`: must exist in the supported model catalog → if not, exclude binding with warning.
     - `provider_id`: must exist in providers → if not, exclude binding.
     - `model_name`: provider-specific name (non-empty).
     - `display_name`: user-facing label. Enabled persisted bindings must provide it. If absent in a legacy or invalid config source, exclude the binding and report a repair suggestion using the supported model's display name.
     - `invocation_method`: supported by the program (OpenAI Responses, OpenAI Chat Completions, Anthropic Messages).
     - `reasoning_effort`: allowed by the model's `reasoning_capability` -> if not, exclude the binding with a `needs_configuration` diagnostic. Do not silently clamp persisted configuration.
     - default selection: read from effective `[defaults].model_binding`, not from a binding-local `is_default` flag.
  2. For each valid binding, resolve into a `ResolvedModelProfile`:
     - Merge capability metadata from `SupportedModelDefinition`.
     - Merge connection details from `UserProvider` (base_url, credential).
     - Merge binding details (model_name, display_name, invocation_method, reasoning_effort).
     - Compute `effective_context_window`.
  3. The default binding is the effective `[defaults].model_binding` when valid. If absent, select the first valid binding only as a startup fallback and report that no durable default is configured where user-facing diagnostics are shown.
- **Postconditions**: A list of invocable `ResolvedModelProfile` values is ready. Clients can select any valid binding.

### B4. Session Model Selection

- **Trigger**: Client sends `model/select` or `turn/submit` with model overrides.
- **Preconditions**: The session is loaded. The selected binding is valid.
- **Algorithm / Flow**:
  1. Client may select a `binding_id` from the invocable list.
  2. Client may override `reasoning_effort` (session-local, does not rewrite the binding config).
  3. Persist the selection in session metadata (`active_model_binding`, `session_reasoning_effort`).
  4. If the caller requests durable default persistence, delegate to `L3-BEH-APP-001` and the policy in `L2-DES-APP-002`:
     - Before the first user message, a pending model or reasoning default may be persisted to the selected configuration scope.
     - After the first user message, normal model changes are session state and must not rewrite provider, binding, credential, or default records immediately.
     - Graceful server-exit persistence of active reasoning effort updates only the relevant default reasoning field.
  5. Session model selection is used for the NEXT turn. It does not change the model mid-turn.
- **Postconditions**: The next turn uses the selected model and reasoning effort.

### B5. Provider Request Construction

- **Trigger**: Context assembly is complete and a model invocation is starting.
- **Preconditions**: `ResolvedModelProfile` is ready. Assembled context is available.
- **Algorithm / Flow**:
  1. Based on `invocation_method`:
     - `OpenAiResponses`: use OpenAI Responses API format.
     - `OpenAiChatCompletions`: use Chat Completions format with system/user/assistant/tool messages.
     - `AnthropicMessages`: use Anthropic Messages API format with system parameter and user/assistant messages.
  2. Serialize the immutable prefix, metadata-derived instructions, hidden goal context, change signal, and user input into provider-specific message structures.
  3. Include tool schemas (for OpenAI: `tools` array; for Anthropic: `tools` array).
  4. Set model name from `binding.model_name`.
  5. Set reasoning effort/thinking from `binding.reasoning_effort` (or session override).
  6. Authenticate: resolve `credential_ref` to the actual API key from `auth.json`. Set in `Authorization` header.
  7. Compute `immutable_prefix_hash` and include as a provider cache key if the provider supports prefix caching.
  8. Return the `ProviderRequest` ready for HTTP call.
- **Postconditions**: A provider-native request is built. Credentials are attached but never logged.

### B6. Provider Error Handling

- **Trigger**: Provider returns an HTTP error or streaming error.
- **Preconditions**: The provider call was attempted.
- **Algorithm / Flow**:
  1. Classify provider errors:
     - 401/403 → `AuthenticationError` (check credential validity).
     - 429 → `RateLimitError` with `retry_after` header.
     - 5xx → `ProviderServerError` (transient, retryable).
     - Timeout → `ProviderTimeoutError`.
     - Context length exceeded → `ContextLimitError` (trigger emergency compaction).
     - Invalid model name → `ModelNotFoundError`.
     - Billing/quota → `QuotaExceededError`.
  2. Map to structured `ProviderError` with:
     - `code` (machine-readable), `message` (user-facing), `retry_after` (optional seconds), `recoverable` (bool), `retry_state`.
  3. For `ContextLimitError`: attempt emergency compaction, retry once. If still fails → fail the turn.
  4. For `RateLimitError`: wait `retry_after` or a default backoff, retry once.
  5. For `AuthenticationError`: mark provider as `needs_configuration`. Fail the turn with clear message.
- **Postconditions**: Provider errors are classified and actionable recovery steps are exposed.

## Traceability

| L2 Source | Relationship |
|---|---|
| L2-DES-MODEL-001 | specified-by |
| L2-DES-APP-005 | specified-by |
| L3-BEH-APP-001 | related-to |

## Implementation Placement Guidance

- `SupportedModelDefinition` catalog ownership belongs to core; `crates/core/src/model_catalog.rs` is a conventional placement if the module follows this L3 contract.
- `ResolvedModelProfile` is a runtime-only struct; it combines data from three sources and is not persisted directly.
- Provider adapters belong in the provider crate and should be organized by provider family or invocation method.
- Credential resolution from `auth.json` must happen as late as possible (just before the HTTP call) and the resolved value must never be logged.

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-27 | Assistant | Initial | Initial model resolution and provider binding behavior. |
| 2 | 2026-05-27 | Assistant | Correction | Aligned display-name validation, reasoning-effort errors, default selection, and persistence writes with `L3-BEH-APP-001`. |
| 3 | 2026-05-27 | Assistant | Correction | Clarified that provider credential checks use the user-scoped auth store rather than a merged auth view. |

---
artifact_id: L3-BEH-PROVIDER-003
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-27
---

# L3-BEH-PROVIDER-003 - Model Usage Observability

## Purpose

Define implementation behavior for invocation identity, usage metric normalization, context-pressure reporting, durable usage records, safe client usage projections, and trace-mode model stream diagnostics.

## Source Design

- `L2-DES-LLM-003` defines model usage observability requirements.
- `L2-DES-APP-004` defines observability sinks, redaction, trace mode, and telemetry boundaries.
- `L2-DES-CONV-001` defines durable usage records in session JSONL.
- `L2-DES-MODEL-001` defines resolved model profiles and provider bindings.
- `L3-BEH-PROVIDER-002` defines provider stream normalization.
- `L3-BEH-APP-002` defines shared observability runtime behavior.

## Core Types

```rust
pub enum MetricSource {
    ProviderReported,
    LocalEstimate,
    Unavailable,
    Redacted,
}

pub enum MetricConfidence {
    Exact,
    Approximate,
    ProviderDefined,
    Unknown,
}

pub enum MetricInclusion {
    IncludedIn(&'static str),
    SeparateFrom(&'static str),
    Unknown,
}

pub struct UsageMetric {
    pub name: String,
    pub value: Option<u64>,
    pub unit: UsageUnit,
    pub source: MetricSource,
    pub confidence: MetricConfidence,
    pub provider_field: Option<String>,
    pub included_in: MetricInclusion,
    pub notes: Option<String>,
}

pub struct InvocationUsage {
    pub invocation_id: InvocationId,
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub model_binding_id: ModelBindingId,
    pub canonical_model_slug: String,
    pub provider_id: ProviderId,
    pub invocation_method: InvocationMethod,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub metrics: BTreeMap<String, UsageMetric>,
    pub context_pressure: ContextPressure,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub status: InvocationStatus,
}

pub struct ContextPressure {
    pub context_tokens: UsageMetric,
    pub effective_context_window: u64,
    pub pressure_ratio: Option<f64>,
    pub pressure_state: ContextPressureState,
    pub compaction_threshold: Option<f64>,
    pub compaction_status: CompactionStatus,
}
```

## B1. Create Invocation Identity

- **Trigger**: The execution engine is ready to call a model provider.
- **Preconditions**: The resolved model profile and context snapshot are available.
- **Algorithm / Flow**:
  1. Allocate `invocation_id` before the provider request is built.
  2. Allocate `trace_id` when trace mode is enabled.
  3. Attach `invocation_id` to:
     - provider request context
     - structured logs
     - stream trace records
     - durable usage records
     - server-client usage events
     - provider error diagnostics
  4. Emit `model.invocation_started` through `L3-BEH-APP-002`.
- **Postconditions**: All records for one model call can be correlated without parsing content.

## B2. Record Initial Context Pressure

- **Trigger**: Context assembly completes for an invocation.
- **Preconditions**: Effective context window is known from `ResolvedModelProfile`.
- **Algorithm / Flow**:
  1. Read exact context token count if the context assembler has a provider-compatible tokenizer.
  2. Otherwise compute an estimate and mark `source = LocalEstimate`.
  3. Compute `pressure_ratio = context_tokens / effective_context_window` when both values are available.
  4. Map pressure state:
     - `normal`: below compaction threshold.
     - `high`: at or above advisory threshold.
     - `near_limit`: near effective context limit.
     - `over_limit`: above effective context limit.
     - `unknown`: context size is unavailable.
  5. Emit a `context_updated` or usage projection event before model invocation when possible.
- **Postconditions**: User-facing status can show context pressure before the model finishes.

## B3. Normalize Provider Usage

- **Trigger**: Provider stream or non-streaming response includes usage data, or invocation ends without usage data.
- **Preconditions**: A provider adapter owns provider-specific field mapping.
- **Algorithm / Flow**:
  1. Map provider fields into common metrics:
     - `input_tokens`
     - `cached_input_tokens`
     - `output_tokens`
     - `reasoning_tokens`
     - `total_tokens`
  2. Preserve original field name in `provider_field`.
  3. If a provider omits a metric, create that metric with `source = Unavailable` and no value.
  4. If the program estimates a metric locally, use `source = LocalEstimate` and `confidence = Approximate` unless the adapter can prove exact compatibility.
  5. If a value cannot be displayed because of policy, use `source = Redacted`.
  6. Emit `model.usage_received`.
- **Postconditions**: Usage is provider-independent but does not pretend unavailable values are zero.

## B4. Reasoning Token Inclusion Policy

- **Trigger**: The provider reports reasoning tokens or thinking tokens.
- **Preconditions**: Provider-specific usage semantics may or may not identify inclusion relationships.
- **Algorithm / Flow**:
  1. Record `reasoning_tokens` as its own metric when reported.
  2. Set `included_in` only when the provider adapter knows the relationship.
  3. If relationship is unknown, use `MetricInclusion::Unknown`.
  4. Do not derive `total_tokens` by adding reasoning tokens to output tokens unless the adapter proves they are separate.
  5. User-facing totals prefer provider-reported totals. Derived totals must label their source and formula.
- **Postconditions**: The program avoids double-counting reasoning tokens.

## B5. Accumulate Turn And Session Usage

- **Trigger**: Invocation usage changes or invocation completes.
- **Preconditions**: The turn has an active usage accumulator.
- **Algorithm / Flow**:
  1. Store invocation-level metrics separately from turn totals.
  2. For turn/session token counters used by status lines:
     - Read = `input_tokens`.
     - Cached read = `cached_input_tokens`.
     - Write = `output_tokens`.
  3. Preserve metric source labels.
  4. If multiple invocations occur in one turn, accumulate only compatible numeric metrics and preserve per-invocation details.
  5. Do not add unavailable values as zero unless display explicitly says unavailable and excludes them from totals.
- **Postconditions**: Turn and session usage remain accurate and explainable.

## B6. Write Durable Usage Records

- **Trigger**: Usage is received, invocation completes, or a turn ends without provider usage.
- **Preconditions**: `SessionStore` is available.
- **Algorithm / Flow**:
  1. Append `UsageRecorded` to session JSONL with:
     - `session_id`
     - `turn_id`
     - `invocation_id`
     - model and provider identifiers
     - normalized metrics
     - context pressure
     - metric source labels
     - `recorded_at`
  2. Avoid writing prompt content, response content, request headers, or credential material.
  3. If provider usage arrives after text completion but before final invocation completion, write usage before `TurnCompleted` where possible.
  4. If no usage data is available, write unavailable metrics so replay can distinguish "unknown" from "zero".
- **Postconditions**: Session replay can reconstruct usage summaries without trace logs.

## B7. Emit Safe Client Usage Projections

- **Trigger**: Usage or context pressure changes.
- **Preconditions**: One or more clients are subscribed.
- **Algorithm / Flow**:
  1. Build a protocol projection with:
     - invocation usage
     - turn delta
     - session totals
     - context pressure
     - source labels
     - unavailable/redacted markers
  2. Send through `turn/usage/updated` or another concrete turn-scoped notification according to the protocol design.
  3. Keep provider-specific raw fields out of the projection unless they are safe diagnostic metadata.
- **Postconditions**: TUI and other clients do not need provider-specific token parsing.

## B8. Trace Model Stream Events

- **Trigger**: Provider stream normalization emits a stream event and trace mode is enabled.
- **Preconditions**: `trace_id` and `invocation_id` exist.
- **Algorithm / Flow**:
  1. Assign a monotonic trace sequence within the invocation.
  2. Record:
     - provider event kind
     - normalized event kind
     - timestamp
     - elapsed milliseconds
     - content policy
     - byte and character counts
     - finish reason where available
     - usage fragment where available
     - error fragment where available
  3. Apply trace content policy from `L3-BEH-APP-002`.
  4. Store trace records outside durable session JSONL.
- **Postconditions**: Stream ordering and timing can be investigated without making traces part of transcript replay.

## B9. Provider Adapter Requirements

Each provider adapter must implement a usage normalizer for every supported invocation method.

Adapter behavior:

- Map known provider fields to common metrics.
- Preserve field names.
- Mark missing values unavailable.
- Preserve provider-defined confidence.
- Explicitly document reasoning-token inclusion relationships when known.
- Emit provider error category and recovery hint through the observability runtime.
- Never compute monetary cost unless a later pricing requirement defines pricing inputs.

## B10. Required Tests

- Provider-reported input, cached input, output, and total tokens are normalized with provider field names.
- Missing cached input usage is `Unavailable`, not zero.
- Locally counted context tokens are marked `LocalEstimate`.
- Unknown reasoning-token inclusion does not affect derived totals.
- Known reasoning-token inclusion is preserved in the metric.
- Durable `UsageRecorded` contains no prompt, response, headers, or credentials.
- Replay reconstructs invocation, turn, and session usage summaries from `UsageRecorded`.
- Client projection uses read/cached-read/write naming and source labels.
- Trace `metadata_only` records preserve event ordering and omit streamed text.
- Provider failure still records invocation id and unavailable usage where appropriate.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| specifies | L2-DES-LLM-003 | 1 | specs/L2/llm/L2-DES-LLM-003-model-usage-observability.md | Implements invocation identity, normalized usage metrics, context pressure, durable usage, client projections, and trace-mode stream records. |
| related-to | L2-DES-APP-004 | 1 | specs/L2/app/L2-DES-APP-004-observability-architecture.md | Uses observability sinks, redaction, trace mode, and telemetry boundaries. |
| related-to | L2-DES-CONV-001 | 1 | specs/L2/conv/L2-DES-CONV-001-session-jsonl-data-model.md | Writes durable usage records to session JSONL. |
| related-to | L2-DES-MODEL-001 | 2 | specs/L2/model/L2-DES-MODEL-001-model-provider-binding.md | References resolved model and provider identity. |
| related-to | L3-BEH-PROVIDER-002 | 1 | specs/L3/provider/L3-BEH-PROVIDER-002-stream-normalization.md | Consumes normalized provider stream events. |
| related-to | L3-BEH-APP-002 | 1 | specs/L3/app/L3-BEH-APP-002-observability-runtime.md | Emits logs, traces, and client diagnostics through the observability runtime. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-27 | Assistant | Initial | Initial L3 model usage observability behavior. |

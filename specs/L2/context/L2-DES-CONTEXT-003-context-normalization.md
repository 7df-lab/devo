---
artifact_id: L2-DES-CONTEXT-003
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-22
---

# L2-DES-CONTEXT-003 — Context Normalization

## Purpose

Define how the program normalizes assembled model context before provider serialization, bounding individual item sizes, preserving tool-call pairing integrity, filtering unsupported modalities against the current model's capabilities, and ensuring the total context fits within the model's effective token budget.

## Background / Context

`L2-DES-CONTEXT-001` defines context assembly — how instructions, metadata, transcript items, and change-signal messages compose into model-visible context. `L2-DES-CONTEXT-002` defines compaction — how older history is summarized when the context grows too large. Neither defines what happens when individual items are oversized, modality-incompatible, or when the total assembled context still exceeds the model's limits after compaction.

Normalization is the final safety pass between context assembly and provider serialization. It ensures that the context sent to any model is well-formed, bounded, modality-compatible, and deterministic.

## Source Requirements

- `L1-REQ-CONTEXT-002` requires item size bounding, visible truncation, tool-call pairing integrity, modality filtering against the current model, and model-switching safety.
- `L1-REQ-CONTEXT-001` requires useful model context that stays within limits.
- `L1-REQ-LLM-001` requires token-efficient context construction.
- `L1-REQ-MODEL-001` requires model capability metadata used for modality filtering.
- `L1-REQ-INPUT-001` requires multimodal content parts as first-class context.
- `L2-DES-CONTEXT-001` defines the assembled context that normalization receives as input.
- `L2-DES-CONTEXT-002` defines compaction, which runs before normalization.
- `L2-DES-MODEL-001` defines the resolved model profile used for modality compatibility checks.
- `L2-DES-AGENT-001` defines the execution engine phase where normalization runs.

## Design Requirement

The program should normalize assembled context immediately before each model invocation. Normalization runs after context assembly and compaction, and before provider-specific serialization into request messages.

Normalization applies three passes in order:

1. **Modality filter**: Remove or convert content parts unsupported by the current model.
2. **Item size bound**: Truncate individual oversized items with visible truncation indicators.
3. **Token-budget bound**: If the total estimated token count still exceeds the model's effective budget, apply a budget-aware reduction that preserves the most recent and most important content.

Normalization is deterministic: given the same assembled context and the same resolved model profile, it must produce the same normalized output.

## Execution Phase

Normalization runs between compaction and provider serialization in the execution engine's flow:

```text
Context assembly (per L2-DES-CONTEXT-001)
        ↓
Compaction check (per L2-DES-CONTEXT-002)
        ↓
Normalization (this design)           ← runs here
        ↓
Provider serialization (system/developer/user/assistant/tool messages)
        ↓
Model invocation
```

Normalization runs before every model invocation, including within tool-call loops where context carries new tool results. When the user switches models mid-session, normalization runs with the new model's capabilities.

Normalization does not mutate durable transcript records. It produces a normalized projection of the assembled context for the current invocation only. The durable transcript and context snapshot remain unmodified.

## Pass 1 — Modality Filter

The modality filter removes or converts content parts that are incompatible with the current model's supported modalities.

### Filtering Rules

1. Resolve the current model's supported modalities from the `ResolvedModelProfile` → `SupportedModelDefinition.modalities`.
2. For each content part in the assembled context, check whether the part's modality is in the supported set.
3. If the modality is supported, pass the content part through unchanged.
4. If the modality is not supported:
   - If a conversion to a supported modality exists and is enabled, convert the content part.
   - Otherwise, remove the content part from the normalized output.
5. If removing a content part results in an item having no remaining content parts, insert a placeholder text part describing what was removed and why. The placeholder must identify the removed modality and the count of removed parts, but must not include the removed binary payload.

### Modality Categories

| Modality | Content Part Kind | Filter Behavior When Unsupported |
|---|---|---|
| `text` | `text` | Always supported. Pass through. |
| `image` | `image_ref` | Remove unless convertible. |
| `audio` | `audio_ref` | Remove unless convertible. |
| `video` | `video_ref` | Remove unless convertible. |
| `tool_call_json` | `tool_call_json` | Always supported (structural, not modality). Pass through. |
| `tool_result_text` | `tool_result_text` | Always supported. Pass through. |
| `provider_metadata` | `provider_metadata` | Remove (provider-specific and not model-visible). |

Multimodal artifact references (`image_ref`, `audio_ref`, `video_ref`) that point to binary content should not have the binary payload included in the normalized context when the modality is unsupported. The content part is removed from the normalized output and a placeholder note is inserted.

### Model-Switching Safety

When the user switches models between turns, normalization must re-evaluate modality compatibility against the new model. Content parts that were valid for the previous model may become invalid and must be filtered. Content parts that were previously filtered may become valid with the new model and should be restored (since normalization reads from durable content parts, which are unchanged by previous filtering passes).

### Placeholder Format

When content parts are removed, the placeholder text should be concise:

```text
[Unsupported content omitted: 2 image(s), 1 video(s). The current model supports text only.]
```

The placeholder must not include image data, URLs that resolve to the removed content, or other information that should remain in the durable record but outside the model request.

## Pass 2 — Item Size Bound

After modality filtering, individual items are bounded to prevent any single item from consuming a disproportionate share of the context window.

### Per-Item-Type Limits

Different item types have different size policies because their content serves different purposes:

| Item Kind | Limit Strategy | Default Limit | Truncation Note |
|---|---|---|---|
| `user_input` | Truncate tail | High (generous, near context-window fraction) | `[User message truncated: N bytes omitted]` |
| `assistant_text` | Truncate tail | Medium | `[Response truncated: N bytes omitted]` |
| `assistant_reasoning` | Truncate tail | Medium | `[Reasoning truncated: N bytes omitted]` |
| `tool_call` | Must fit entirely | Call arguments are bounded | Tool calls that exceed limit are marked invalid; the model must not see partial tool-call JSON |
| `tool_result` | Truncate tail | Medium | `[Tool output truncated: N bytes omitted from result of {tool_name}]` |
| `error` | Preserve entirely | Full | Errors are not truncated; they carry diagnostic value |
| `steer_message` | Truncate tail | Medium | `[Steer message truncated: N bytes omitted]` |
| `context_summary` | Truncate tail | Medium | `[Summary truncated: N bytes omitted]` |
| `approval_request` | Preserve entirely | Full | Approval context must be intact for user decisions |
| `question_request` | Preserve entirely | Full | Question context must be intact for user understanding |

Limits are configurable per item kind. The default values should be sensible fractions of typical context windows.

### Truncation Behavior

Truncation is always at the tail (end) of the content — content at the beginning is preserved because it typically carries more context-establishing information. Tool results are the exception: for very large tool outputs where the relevant result is at the end, the program may offer a configurable truncation strategy (head, tail, or head-and-tail).

Truncation indicators must be:
- Injected as structured text within the truncated content part.
- Visible to both the model and the user (via client display of normalized context).
- Distinct from the original content so replay does not confuse the indicator with authentic tool output.

Truncation must not:
- Break the structural integrity of tool-call JSON.
- Remove the tool_call_id or tool_name from a tool result (pairing anchors are preserved).
- Remove error codes, error messages, or recovery hints from error items.

### Tool Call Integrity

A tool call from the model must arrive at normalization as a complete, valid JSON structure. If a tool call's serialized arguments exceed the item limit, the tool call must not be partially included — it is marked invalid and replaced with an error note:

```text
[Tool call to {tool_name} omitted: arguments exceed the size limit.]
```

The corresponding tool result must also be omitted to preserve pairing. Orphaned tool calls (without results) and orphaned tool results (without their initiating call) must not appear in the normalized context.

When a tool-call/tool-result pair spans a compaction boundary (the call was in compacted history but the result is recent), the result must still be paired. If the compacted summary does not include the tool call, the result should be treated as a standalone item with a note that its originating call was summarized.

## Pass 3 — Token-Budget Bound

After modality filtering and item size bounding, the total estimated token count may still exceed the model's effective context budget. The token-budget pass applies a budget-aware reduction.

### Token Estimation

The program maintains a token estimate for each content part and for the assembled context as a whole. Estimation may use:
- Provider-reported token counts from previous invocations.
- Character-based heuristics (e.g., characters ÷ N for the provider family).
- Cached token counts from prior normalization passes.

The estimate must be conservative enough that normalization does not produce context that the provider then rejects. It does not need to be byte-exact.

### Budget-Aware Truncation

If the total estimated tokens exceed the effective context budget:

1. Identify the oldest items in the context that are eligible for further reduction.
2. Apply progressive truncation from oldest to newest:
   - First, further truncate tool results in the eligible range.
   - Second, further truncate assistant reasoning in the eligible range.
   - Third, further truncate assistant text in the eligible range.
   - Fourth, truncate user input in the eligible range.
3. Stop when the estimated token count fits within the budget.
4. Instruction content (system instructions, mode instructions, persona instructions, project instruction files) must not be truncated by the token-budget pass. Compaction is the mechanism for reducing instruction-level content.
5. The most recent user input and any in-progress tool work must not be truncated by the token-budget pass.

If truncation cannot bring the context within budget without removing required instruction or current-turn content, the program must produce a structured error indicating that the context cannot be normalized for the current model.

### Visibility

When the token-budget pass truncates content, the program must:
- Emit a `context_updated` event so clients can show the updated token estimate.
- Mark the truncated items in client projections so the user can see what was reduced.
- Log the before-and-after token estimates for debugging.

## Per-Item-Type Policies

Different item types carry different types of content. Normalization should apply type-appropriate strategies:

| Item Kind | Modality Check | Size Bound | Token-Budget Eligible | Notes |
|---|---|---|---|---|
| User input | Check attachments | High limit | Last resort (oldest first) | Preserve user intent. Attachments filtered by modality. |
| Assistant text | Text only | Medium limit | Yes (oldest first) | Preserve decisions and findings. |
| Assistant reasoning | Text only | Medium limit | Yes (oldest first) | Reduce before assistant text. |
| Tool call | Structural only | Must fit entirely | Yes (as part of pair) | Cannot be partially included. |
| Tool result | Text only | Medium limit | Yes (first candidate) | Preserve tool_call_id pairing. |
| Error | Text only | Full | No | Diagnostic value preserved. |
| Context summary | Text only | Medium limit | No | Already compacted; do not re-truncate. |
| Instructions | Text only | Full | No | Must not be truncated by normalization. |
| Change-signal | Text only | Full | No | Already minimal; do not truncate. |

## Interaction With Compaction

Compaction runs before normalization. Compaction reduces the number of transcript items by summarizing older history. Normalization handles what remains:
- Individual oversized items that survived compaction.
- Modality filtering for the current model.
- Token-budget overflow after compaction.

Compaction is the primary mechanism for reducing long-history context. Normalization is the safety net for individual item size, modality compatibility, and total budget overflow. They do not overlap: compaction does not filter modalities; normalization does not summarize history.

A compaction summary is a context item of kind `context_summary`. It receives the same normalization treatment as other items: modality filtering (text only, always supported), size bounding (medium limit), and exemption from token-budget truncation (already compacted).

## Determinism

Normalization must be deterministic for the same inputs:
- Same assembled context (identical items, content parts, and ordering).
- Same resolved model profile (same model slug, same modality set, same effective context window).

Determinism enables:
- Reproducible debugging: when a model call fails with a context error, the normalized context can be reconstructed.
- Client-side context inspection: the client can request a projection of the normalized context and receive a consistent result.
- Replay verification: replaying a turn from durable records should produce the same normalized context for the same model.

External factors such as provider-reported token counts from prior invocations may affect the token estimate, making the budget-bound pass dependent on invocation history. This is acceptable; the estimate is input to normalization, and normalization with the same estimate must produce the same output.

## Invariants

- Normalization does not mutate durable transcript records or context snapshots.
- Tool calls and their results must remain paired. Orphaned calls or results must not reach the model.
- Truncation must be visibly indicated; silent data loss is not allowed.
- Unsupported modality payloads must not be sent to model providers.
- Instruction content must not be truncated by normalization.
- Normalization produces deterministic output for the same assembled context and model profile.
- When the user switches models, normalization re-evaluates against the new model's capabilities.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---:|---|---|---|
| refines | L1-REQ-CONTEXT-002 | 1 | specs/L1/L1-REQ-CONTEXT-002-normalize.md | Defines the concrete normalization pipeline: modality filter, item size bounds, tool pairing integrity, token-budget reduction, and model-switching safety. |
| related-to | L1-REQ-CONTEXT-001 | 1 | specs/L1/L1-REQ-CONTEXT-001-management.md | Normalization ensures context is well-formed and bounded before model invocation. |
| related-to | L1-REQ-LLM-001 | 1 | specs/L1/L1-REQ-LLM-001-token-efficiency.md | Token-budget pass and size bounding prevent wasted tokens from oversized items. |
| related-to | L1-REQ-MODEL-001 | 1 | specs/L1/L1-REQ-MODEL-001-config.md | Model capability metadata drives modality filtering decisions. |
| related-to | L1-REQ-INPUT-001 | 1 | specs/L1/L1-REQ-INPUT-001-attachments-and-multimodal.md | Multimodal content parts are subject to modality filtering. |
| related-to | L2-DES-CONTEXT-001 | 1 | specs/L2/context/L2-DES-CONTEXT-001-context-assembly.md | Normalization receives the assembled context as input. |
| related-to | L2-DES-CONTEXT-002 | 1 | specs/L2/context/L2-DES-CONTEXT-002-context-compaction.md | Normalization runs after compaction and handles items that survive compaction. |
| related-to | L2-DES-MODEL-001 | 2 | specs/L2/model/L2-DES-MODEL-001-model-provider-binding.md | The resolved model profile provides modality capabilities and context-window limits. |
| related-to | L2-DES-AGENT-001 | 1 | specs/L2/agent/L2-DES-AGENT-001-execution-engine.md | Normalization runs as a phase of the execution engine between compaction and provider serialization. |
| specified-by | L3-BEH-CORE-005 | 2 | specs/L3/core/L3-BEH-CORE-005-context-pipeline.md | L3 defines modality filtering, item size bounding, and token-budget enforcement. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-22 | Assistant | Initial | Initial context normalization design covering three-pass pipeline, per-item-type policies, modality filtering, tool pairing integrity, token-budget bound, and determinism. |

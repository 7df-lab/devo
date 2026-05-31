---
artifact_id: L2-DES-CLIENT-001
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-23
---

# L2-DES-CLIENT-001 — Localization Readiness

## Purpose

Refine client localization readiness into a technical design for Unicode-safe input, IME-safe editing, wide-character rendering, localized content preservation, and future UI string translation.

## Background / Context

The first product milestone may ship English UI strings, but clients must already handle multilingual user content. User prompts, file paths, provider messages, tool output, command output, and transcript records may contain Unicode and localized text. A client that corrupts non-ASCII input or CJK display width is not usable for many users even if the UI chrome remains English.

## Source Requirements

- `L1-REQ-CLIENT-001` requires client interfaces to accept and display Unicode input, support IME composition in supported environments, preserve non-ASCII paths/content, handle CJK and wide-character layout, and structure user-visible strings for future localization.
- `L1-REQ-TUI-001` requires the composer to preserve non-ASCII and IME input.
- `L1-REQ-TUI-003` requires readable transcript display.
- `L1-REQ-TUI-007` requires responsive layout and readability.
- `L2-DES-TUI-002` defines modern TUI layout constraints.
- `L2-DES-TUI-003` defines composer behavior.
- `L2-DES-TUI-004` defines transcript rendering.

## Design Requirement

Client interfaces should separate text content handling from UI string localization.

The initial design must guarantee locale-safe handling for user and runtime content even before translated UI catalogs exist. Future localization should be enabled by structuring UI strings and formatting logic so translation can be added without rewriting workflows.

## Text Categories

The program should distinguish these text categories:

| Category | Examples | Handling |
|---|---|---|
| User content | Prompts, pasted text, file mentions. | Preserve exactly as user-authored. |
| Runtime content | Tool output, provider responses, errors from external commands. | Preserve content, apply redaction and display bounds only where required. |
| UI chrome | Labels, buttons, status text, hints. | Centralize or structure for future translation. |
| Protocol identifiers | Method names, enum values, artifact ids. | Keep stable and not localized. |
| Diagnostic codes | Error codes, metric names, machine states. | Keep stable; localize accompanying messages later. |

## Unicode Text Model

Clients should use a Unicode-aware text model.

Rules:

- Store and transport text as UTF-8 or another explicitly Unicode-safe representation.
- Do not split visible text by raw bytes.
- Cursor movement should operate on grapheme clusters where editing is visible to users.
- Display width calculations should use terminal/display column width, not scalar count or byte length.
- CJK wide characters, emoji, combining marks, and zero-width joiner sequences should not corrupt cursor position or wrapping.
- Truncation should occur at grapheme boundaries and should account for display width.
- Redaction should preserve valid Unicode output.

## IME Composition

Clients that support editable text should treat IME composition as an input state.

Rules:

- In-progress composition text should not be submitted as final user input.
- Composition updates should not trigger command-prefix execution.
- The composer should preserve committed IME text exactly.
- Cursor and selection state should remain coherent after composition commits.
- If a terminal or environment cannot expose enough composition information, the client should degrade predictably and document the support limitation.

## Terminal Width And Wrapping

TUI rendering must account for display columns.

Rules:

- Wrapping should use display width rather than bytes.
- Cell borders, status labels, and truncation markers should not split wide characters.
- A line ending with a wide character should not overflow into the next region.
- Horizontal truncation should reserve display columns for an omission marker when used.
- Status lines should collapse optional metadata before truncating important mode or waiting-state labels.

Example:

```text
Incorrect:
> 修复 parser 的 quote
| status overwrites the final wide char |

Correct:
> 修复 parser 的 quote
--------------------------------------------------------------------------------
ready                                                     Plan
```

## Non-ASCII Paths And Mentions

File paths and mentions may contain non-ASCII text.

Rules:

- Display paths without lossy conversion.
- Preserve path text through client/server protocol calls.
- Avoid normalizing path strings in a way that changes filesystem meaning.
- When truncating long paths, keep enough leading or trailing context to identify the path.
- If a path cannot be displayed safely, show a safe escaped representation rather than corrupting layout.

## UI String Structure

The initial product may ship English UI strings only. Even so, client code should avoid scattering hard-coded user-facing strings through business logic.

Recommended structure:

- Keep user-facing strings near view or presentation layers.
- Use stable message keys or structured message constructors where practical.
- Keep diagnostic machine codes separate from human display text.
- Avoid concatenating translated fragments in ways that would block future localization.
- Allow labels to expand in future languages without breaking layout assumptions.
- Use formatted values through typed placeholders rather than manual string splicing.

Example conceptual structure:

```text
message_key: tui.status.waiting_for_tool
values:
  tool_name: cargo test
  elapsed: 00:04
fallback_en: Waiting for tool: cargo test 00:04
```

## Locale-Safe Diagnostics

Diagnostic display should remain clear with localized external output.

Rules:

- Provider and tool output may be non-English and should be preserved.
- User-facing recovery hints may start as English but should be isolated for future translation.
- Error codes remain stable and not localized.
- Logs should preserve valid Unicode after redaction.

## Test Strategy

L3 and implementation should include targeted tests for:

- Non-ASCII composer input submission.
- IME committed text preservation where testable.
- CJK display width in composer, transcript, and status lines.
- Truncation at grapheme boundaries.
- Non-ASCII file path display.
- Tool output containing localized text.
- UI string structure that separates codes from display text.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-CLIENT-001 | 1 | specs/L1/L1-REQ-CLIENT-001-localization-readiness.md | Defines Unicode, IME, display-width, non-ASCII path, diagnostic, and future localization design. |
| related-to | L1-REQ-TUI-001 | 1 | specs/L1/L1-REQ-TUI-001-composer.md | Composer input must preserve Unicode and IME text. |
| related-to | L1-REQ-TUI-003 | 1 | specs/L1/L1-REQ-TUI-003-transcript.md | Transcript rendering must preserve localized and non-ASCII content. |
| related-to | L1-REQ-TUI-007 | 1 | specs/L1/L1-REQ-TUI-007-responsive-layout-readability.md | Responsive layout depends on display-width aware rendering. |
| related-to | L2-DES-TUI-002 | 1 | specs/L2/tui/L2-DES-TUI-002-modern-tui-shell-layout.md | The shell layout must account for Unicode and localized text expansion. |
| related-to | L2-DES-TUI-003 | 1 | specs/L2/tui/L2-DES-TUI-003-composer-and-input-modes.md | Composer and input modes rely on Unicode-safe editing. |
| related-to | L2-DES-TUI-004 | 1 | specs/L2/tui/L2-DES-TUI-004-streaming-transcript-and-state.md | Transcript and streaming cells rely on Unicode-safe rendering. |
| specified-by | L3-BEH-CLIENT-001 | 2 | specs/L3/client/L3-BEH-CLIENT-001-connection-subscription.md | L3 defines UTF-8, wide-character, IME, and grapheme handling requirements for clients. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-23 | Assistant | Initial | Initial client localization-readiness design. |

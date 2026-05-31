---
artifact_id: L2-DES-CLIENT-002
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-26
---

# L2-DES-CLIENT-002 — Prefixed Input Actions

## Purpose

Refine the `@` fuzzy-search prefix requirement into a client interaction design for opening search, updating results, confirming selections, escaping literal input, and producing structured mentions.

## Background / Context

Users need to reference files, skills, and MCP capabilities without leaving the composer. The `@` prefix is a compact entry point into fuzzy search. It must be predictable enough that users can tell when they are searching versus writing normal chat text.

The TUI-specific `!` terminal-command prefix is separate and remains governed by TUI requirements. This design covers the general client `@` fuzzy-search prefix.

## Source Requirements

- `L1-REQ-CLIENT-004` requires client input beginning with `@` to route to fuzzy search.
- `L1-REQ-APP-006` requires fuzzy search for project files, skills, MCP capabilities, and other user-facing entities.
- `L1-REQ-CLIENT-001` requires Unicode and IME-safe input behavior.
- `L1-REQ-TUI-008` defines separate TUI-only terminal-command prefix behavior.
- `L2-DES-APP-006` defines fuzzy-search providers and project file search.
- `L2-DES-CLIENT-001` defines Unicode, IME, and display-width constraints.

## Design Requirement

Clients should implement `@` as an input-mode transition at the first character of the composer.

When active, the prefix opens a fuzzy-search popup. Query text after `@` updates search results in real time. Confirming a selected result inserts a structured mention into the composer and returns the composer to normal chat editing.

The text immediately following `@` is always the fuzzy-search query keyword. Users do not type a provider or result type such as `file`, `skill`, or `mcp` after `@`; clients group returned results by type instead.

## Prefix Recognition

Rules:

- `@` triggers prefixed input only when it is the first character of the composer buffer.
- Leading whitespace before `@` does not trigger prefixed input behavior.
- `\@` at the first character escapes a literal leading `@`; the backslash is removed when submitted as normal chat text.
- Prefix recognition must occur only after text is committed. IME composition updates must not trigger fuzzy search.
- Text immediately following `@` must not be parsed as a provider selector or result-type selector.
- If the user edits text before the leading `@`, the client should close the popup and return to normal chat mode.
- TUI `!` prefix handling has higher specificity only for TUI Shell Mode and must not be conflated with `@`.

## Interaction Flow

Initial trigger:

```text
┃ @
  Skills
    build-web-apps
    openai-docs
  MCP
    github:search_repositories
  Files
    crates/file-search/src/lib.rs
```

Query update:

```text
┃ @sea
  Skills
    search-docs
  MCP
    github:search_repositories
  Files
    crates/file-search/src/lib.rs
    specs/L1/L1-REQ-APP-006-fuzzysearch.md
```

Selection confirmation:

```text
before
┃ @sea

after confirming crates/file-search/src/lib.rs
┃ @lib.rs
```

The rendered mention may be a chip, styled token, or plain inline text depending on client capabilities. The underlying composer state should retain a structured mention reference.

## Result Grouping

The popup should group or order results in this order:

1. Skills.
2. MCP entries.
3. Files in the current working directory or active workspace.
4. Optional future providers such as sessions, transcript entries, or commands.

Within each group, provider-specific ranking is allowed. For files, the project file search backend should provide path-aware fuzzy ranking.

The rendered popup should not include a literal title such as `popup`. Type labels may be rendered as compact section headers, but result type sections should not be separated by blank spacer rows.

## Query And Search Session Lifecycle

Rules:

- Opening the popup creates a search session or subscribes to an existing suitable search source.
- Each committed query change after `@` sends a query update using the full text after `@` as the search keyword.
- Provider selection is not encoded in the query text. The client or server may choose provider groups by context, configuration, or search state.
- Newer query updates supersede older query results.
- The client must ignore stale snapshots whose query or session id no longer matches the active popup state.
- Closing the popup cancels or releases the active search session.
- Search results should update incrementally as provider snapshots arrive.
- Search failures should show a concise unavailable or degraded state rather than falling back to normal submission silently.

## Selection Behavior

Rules:

- Enter confirms the currently focused result while the popup is open.
- TUI renderers should mark the focused result with `>`. Prefix-search results normally do not use `●` because they are inserted references, not currently enabled options.
- Confirmation inserts a structured mention into the composer.
- Confirmation does not submit the chat turn by itself.
- After confirmation, the composer returns to normal chat editing with the mention included.
- If no result is selected, Enter should not submit a normal chat turn accidentally while the popup is open.
- The selected result must include enough type information for later context assembly, such as skill, MCP entry, file, session, command, or transcript reference.

## Mention Model

Confirmed results become structured mentions.

Conceptual mention fields:

| Field | Purpose |
|---|---|
| `mention_id` | Client-generated or server-confirmed stable mention id. |
| `kind` | Skill, MCP, file, session, command, or transcript. |
| `display_text` | Text shown in the composer. |
| `target_ref` | Structured target reference. |
| `source_range` | Composer range occupied by the mention. |
| `resolution_status` | Resolved, unavailable, stale, or permission-blocked. |

The visible composer text is not the only source of truth. Submission should include both content text and structured mention data.

## Keyboard And Pointer Behavior

L2 requires only minimal shared behavior:

- Enter confirms the selected result.
- Escape closes the popup without confirming.
- Pointer or touch selection may confirm or focus a result where supported.
- Arrow-key navigation, paging, fuzzy command palette shortcuts, and client-specific styling are L3 or client-specific details.

If Escape closes the popup, the client should preserve typed text in the composer and avoid immediately reopening the popup until the user edits the prefix again or explicitly restarts search.

## Safety And Permissions

Rules:

- The client must not display search results it is not authorized to show.
- Permission-blocked results may be omitted or shown as unavailable according to provider policy.
- Selecting a result must not grant new permissions by itself.
- MCP and skill results should show enough type/source context to prevent accidental selection.
- File results should use workspace-relative paths where possible.
- Secret values must not appear in result labels, previews, or mention metadata sent to clients.

## Unicode And IME

Rules:

- Query text after `@` must preserve Unicode.
- Matching and highlighting should operate on valid Unicode text.
- Display truncation must not split grapheme clusters.
- IME composition must not trigger search until committed.
- Mention insertion must preserve non-ASCII paths and display text.

## Protocol Boundary

The client may use local search providers or server-owned providers depending on client type. When search crosses server-owned state, the client should use server APIs rather than scanning state independently.

Conceptual client-server methods:

| Method | Purpose |
|---|---|
| `search/start` | Create a search session for one or more provider groups. |
| `search/update` | Update the query for an active search session. |
| `search/cancel` | Cancel or release an active search session. |

Conceptual server events:

| Event | Purpose |
|---|---|
| `search/updated` | Send a result snapshot for the active query. |
| `search/completed` | Tell the client all active providers have completed for the current query. |
| `search/failed` | Tell the client the search is unavailable or degraded. |

Search events are live UI projections. They are not transcript records.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-CLIENT-004 | 1 | specs/L1/L1-REQ-CLIENT-004-prefixed-input-actions.md | Defines `@` prefix recognition, popup lifecycle, grouping, selection, escaping, mention creation, and search protocol boundaries. |
| related-to | L1-REQ-APP-006 | 1 | specs/L1/L1-REQ-APP-006-fuzzysearch.md | The prefix action consumes fuzzy-search providers. |
| related-to | L1-REQ-CLIENT-001 | 1 | specs/L1/L1-REQ-CLIENT-001-localization-readiness.md | Prefix input and query text must be Unicode and IME safe. |
| related-to | L1-REQ-TUI-008 | 1 | specs/L1/L1-REQ-TUI-008-terminal-command-prefix.md | TUI terminal-command prefix behavior remains separate from `@`. |
| related-to | L2-DES-APP-006 | 1 | specs/L2/app/L2-DES-APP-006-fuzzy-search-architecture.md | Defines fuzzy-search provider and project file search behavior. |
| related-to | L2-DES-CLIENT-001 | 1 | specs/L2/client/L2-DES-CLIENT-001-localization-readiness.md | Defines Unicode and IME constraints used by prefix recognition. |
| specified-by | L3-BEH-TUI-001 | 2 | specs/L3/tui/L3-BEH-TUI-001-layout-composer-input.md | L3 defines composer prefix handling for slash commands, shell mode, and fuzzy search. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-25 | Assistant | Initial | Initial prefixed input design for `@` fuzzy search, selection, mention insertion, escaping, provider grouping, and protocol boundaries. |
| 1 | 2026-05-25 | Human | Refinement | Clarified that text immediately after `@` is the query keyword, not a provider or result-type selector. |
| 1 | 2026-05-26 | Human | Refinement | Clarified TUI marker semantics for prefix-search result focus and avoided using `●` for inserted-reference results. |

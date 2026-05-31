---
artifact_id: L3-BEH-CORE-010
revision: 1
status: Draft
active_baseline: no
---

# L3-BEH-CORE-010 — Fuzzy Search

## Purpose

Define the concrete behavior for incremental project file search with background indexing, fuzzy matching, cancellation, and result snapshots suitable for interactive client popups and tool-driven search.

## Source Design

L2-DES-APP-006 (Fuzzy Search Architecture)

## Behavior Specification

### B1. Search Session Lifecycle

- **Trigger**: Client sends `search/start` or a client-side popup opens (e.g., user types `@` in composer).
- **Preconditions**: The workspace is accessible. Search roots are configured.
- **Algorithm / Flow**:
  1. Create a `FileSearchSession` with a unique `search_id`.
  2. Determine search roots: the workspace root(s), plus any configured additional search roots.
  3. Spawn two background worker tasks (tokio):
     - **Walker**: recursively walks search roots, applies ignore/exclude rules, pushes discovered relative paths to the matcher.
     - **Matcher**: holds the fuzzy matcher instance (`nucleo`), receives paths from walker, applies current query.
  4. The session remains live until the client sends `search/cancel`, the popup closes, or the session is dropped.
- **Postconditions**: A live search session exists with background workers indexing the workspace.

### B2. Incremental Walk and Indexing

- **Trigger**: Walker worker starts.
- **Preconditions**: Search roots are valid directories.
- **Algorithm / Flow**:
  1. Use the `ignore` crate's `WalkBuilder` configured with:
     - Hidden files: included by default (subject to gitignore rules).
     - Gitignore rules: respected by default.
     - Symlinks: followed where policy permits (configurable, default off).
     - Explicit exclude patterns: translated into walker-level negative overrides.
  2. Walk each root. For each discovered file:
     a. Convert to workspace-relative path.
     b. Push to the matcher's index via a channel (`mpsc::unbounded`).
     c. Check cancellation flag periodically (every 100 files).
  3. After walk completes: send `WalkComplete` signal to matcher.
  4. Maximum indexed paths: `max_indexed_files` (default 100000). Walker stops at the limit.
- **Postconditions**: All eligible files are indexed. The matcher has the full file set.

### B3. Fuzzy Matching and Snapshots

- **Trigger**: Client sends `search/update` with a new query, or walker pushes new paths.
- **Preconditions**: Matcher is initialized with `nucleo::Matcher` configured for path-aware matching.
- **Algorithm / Flow**:
  1. On query update:
     a. Parse the query: case-insensitive, Unicode-normalized (NFD + strip diacritics).
     b. If new query extends the previous query (prefix match), use incremental pattern parsing to avoid re-scanning.
     c. Set the new pattern on the matcher. `nucleo` computes match scores incrementally.
  2. On index update (new paths from walker): matcher incorporates new entries. Match scores update.
  3. Debounce: matcher ticks at ~10ms intervals. Each tick produces a bounded result snapshot (max `limit` results, default 20).
  4. Each snapshot includes: ranked matches with `path`, `match_indices` (for highlighting), and `score`.
  5. Snapshots are sent to the client via `SessionReporter.on_update`.
  6. When the walker completes AND all pending index updates are processed: send `on_complete` with the final result set.
- **Postconditions**: Client receives incremental result snapshots suitable for real-time popup display.

### B4. Query Cancellation and Superseding

- **Trigger**: Client sends `search/update` with a new query while a previous query is still being processed.
- **Preconditions**: A prior `QueryUpdated` signal was sent.
- **Algorithm / Flow**:
  1. The new query supersedes the prior query. The matcher applies the new pattern.
  2. If the prior query's snapshot was not yet emitted: skip it. Only emit snapshots for the latest query.
  3. A `query_revision` counter increments on each `search/update`. Snapshots carry the revision so clients can ignore stale results.
- **Postconditions**: Only results for the current query are shown. Stale snapshots are discarded.

### B5. Shutdown and Resource Cleanup

- **Trigger**: Client sends `search/cancel`, popup closes, or the search session is dropped.
- **Preconditions**: A search session is active.
- **Algorithm / Flow**:
  1. Set the cancellation flag. Walker checks the flag and exits at the next yield point.
  2. Drain remaining queued paths from the matcher channel (don't process — discard).
  3. Drop the `nucleo` matcher (frees index memory).
  4. Worker tasks exit. The session is unregistered.
- **Postconditions**: All background resources are freed. No orphaned worker tasks.

### B6. Search Provider Model

- **Trigger**: Search is used beyond file search — for skills, MCP entries, sessions, commands.
- **Preconditions**: Provider slots are registered.
- **Algorithm / Flow**:
  1. The search infrastructure supports multiple `SearchProvider` implementations:
     - `FileSearchProvider`: project file search (B1-B5).
     - `SkillSearchProvider`: skill catalog search by name/description.
     - `McpSearchProvider`: MCP capability search.
     - `SessionSearchProvider`: recent session search.
     - `CommandSearchProvider`: slash command search.
  2. Each provider implements: `search(query, limit) → Vec<SearchResult>`.
  3. `search/start` may specify `providers` to restrict which providers are active. Default: all configured providers.
  4. Each provider result carries a `provider_group` type so clients render results distinctly (file icon for files, skill icon for skills, etc.).
- **Postconditions**: Fuzzy search works across multiple entity types with consistent API.

## Traceability

| L2 Source | Relationship |
|---|---|
| L2-DES-APP-006 | specified-by |

## Implementation Placement Guidance

- The file-search worker implementation may live in a dedicated `file-search` crate using `nucleo` for fuzzy matching and `ignore` for directory walking.
- The `SearchProvider` trait belongs to the core-facing API used by server and clients.
- Search sessions are connection-local (not cross-client broadcast) per L2-DES-APP-003.
- Match score from `nucleo` uses path-aware scoring: filename matches score higher than directory-only matches.

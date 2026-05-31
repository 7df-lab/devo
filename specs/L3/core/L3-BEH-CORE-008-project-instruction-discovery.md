---
artifact_id: L3-BEH-CORE-008
revision: 2
status: Draft
active_baseline: no
---

# L3-BEH-CORE-008 — Project Instruction File Discovery

## Purpose

Define the concrete behavior for discovering, reading, and assembling project instruction files (`AGENTS.md`, `AGENTS.override.md`, and configured fallbacks) along the directory hierarchy from project root to current working directory, plus global user-level instruction files.

## Source Design

L2-DES-WORKSPACE-001 (Project Instruction File Discovery)

## Behavior Specification

### B1. Project Root Detection

- **Trigger**: Session is created or workspace is refreshed.
- **Preconditions**: The current working directory (`cwd`) is known and canonicalized.
- **Algorithm / Flow**:
  1. Start at the canonicalized `cwd`.
  2. Walk upward through parent directories (including `cwd` itself).
  3. At each ancestor, check for the presence of any configured project-root marker. Default markers: `[".git"]`.
  4. A marker matches when a directory entry with that name exists at that path (file or directory).
  5. Stop at the first ancestor containing any marker. That ancestor is the project root.
  6. If no ancestor contains a marker (reached filesystem root): the project has no discoverable root. Only the `cwd` directory itself is searched for instruction files.
- **Postconditions**: The project root is identified or confirmed absent. The ancestor chain from root to `cwd` is known.

### B2. Per-Directory Instruction File Discovery

- **Trigger**: Project root is identified.
- **Preconditions**: The ancestor chain from project root to `cwd` is known (inclusive).
- **Algorithm / Flow**:
  1. For each directory in the chain, in root-to-cwd order:
     a. Check for `AGENTS.override.md`. If it exists AND is a regular file AND is non-empty (non-whitespace) → use this file, skip remaining priorities for this directory.
     b. If override not found: check for `AGENTS.md`. If it exists, is a regular file, and is non-empty → use this file, skip fallbacks.
     c. If neither found: check configured fallback filenames in order (default: `["CLAUDE.md", "PROMPT.md"]`). First existing, non-empty regular file wins.
     d. If no instruction file found in this directory: contribute nothing from this directory.
  2. Concatenate discovered file contents in root-to-cwd order, separated by double newlines.
  3. Track which files contributed and their paths for diagnostics.
- **Postconditions**: The assembled instruction content is ready for context injection.

### B3. Global Instruction File Inclusion

- **Trigger**: After project instruction discovery.
- **Preconditions**: The user-level configuration directory may contain global instruction files.
- **Algorithm / Flow**:
  1. Resolve the user-level configuration directory:
     - macOS/Linux: `~/.devo/`
     - Windows: `%USERPROFILE%\.devo\`
  2. Check only `AGENTS.override.md`, then `AGENTS.md`, in that directory.
  3. If either file exists, is a regular file, and is non-empty, include it before project instruction content.
  4. Do not check fallback filenames at the global level.
  5. Missing global directory or missing global instruction files is normal and produces no warning.
- **Postconditions**: Global instructions are included in the assembled instruction set.

### B4. Size Bounding

- **Trigger**: Instruction files are read.
- **Preconditions**: Individual files or the total assembly may be large.
- **Algorithm / Flow**:
  1. Apply the configured total assembled byte limit after global and project instruction files are ordered.
  2. If the assembled content exceeds the limit, truncate from the end of the last contributing file and add a truncation notice.
  3. Earlier global and root-level files should remain intact where possible; the bound is applied to the final assembled sequence rather than by arbitrary per-file caps.
  4. Empty files (whitespace-only) are treated as absent and do not consume the limit.
- **Postconditions**: Instruction content is bounded and safe for context assembly.

### B5. Refresh Behavior

- **Trigger**: File watcher detects a change in any discovered instruction file, or user requests refresh.
- **Preconditions**: File watcher is active on all discovered instruction files and their parent directories.
- **Algorithm / Flow**:
  1. On change event (debounced: 500ms after last change): re-run discovery.
  2. Re-read only changed files (not the entire chain).
  3. Re-assemble instruction content.
  4. Update the session's `instruction_set` metadata.
  5. The next turn's context assembly picks up the new instructions.
- **Postconditions**: Instruction changes take effect on the next turn without restart.

### B6. Diagnostics and Visibility

- **Trigger**: Discovery runs or client requests config inspection.
- **Preconditions**: Discovery results are available.
- **Algorithm / Flow**:
  1. Record diagnostics:
     - Project root path (or "none").
     - Each discovered file: path, priority level, size, whether truncated.
     - Directories that contributed no file.
     - Global instruction file path (if configured).
  2. Expose through `config/inspect` with `scope: workspace_instructions`.
  3. If no instruction files found anywhere: this is normal, not an error. The session proceeds with only base instructions.
- **Postconditions**: Users can understand which instruction files are active.

## Traceability

| L2 Source | Relationship |
|---|---|
| L2-DES-WORKSPACE-001 | specified-by |

## Implementation Notes

- Discovery runs at session creation and on refresh. Cache results to avoid repeated filesystem scans.
- File watcher uses `notify` crate. Watch the ancestor chain directories, not the entire workspace tree.
- Default fallback filenames include `CLAUDE.md` to support projects that already maintain Claude Code instruction files.
- The assembled instruction content feeds into context assembly (`L3-BEH-CORE-005`) as part of the `instruction_set` metadata field.

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-27 | Assistant | Initial | Initial project instruction discovery behavior. |
| 2 | 2026-05-27 | Assistant | Correction | Aligned global instruction lookup, size bounding, and context assembly references with L2. |

---
artifact_id: L2-DES-WORKSPACE-001
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-22
---

# L2-DES-WORKSPACE-001 — Project Instruction File Discovery

## Purpose

Define how the program discovers, reads, and assembles project instruction files along the directory hierarchy from the project root to the current working directory, plus global user-level instruction files.

## Background / Context

`L1-REQ-WORKSPACE-002` defines the requirement: discover instruction files on the linear path from project root to cwd, using a per-directory filename priority of `AGENTS.override.md`, `AGENTS.md`, and configurable fallbacks, with root-to-cwd concatenation and no arbitrary depth limit. This design defines the concrete discovery algorithm, filename resolution, global-instruction support, size bounding, configuration surface, and refresh behavior.

The assembled instruction content feeds into `L2-DES-CONTEXT-001` as part of the instruction hierarchy (metadata-derived content, not transcript turns).

## Source Requirements

- `L1-REQ-WORKSPACE-002` requires linear ancestor-chain discovery, per-directory filename priority, configurable fallback filenames, global instruction files, size bounding, and no artificial depth limit.
- `L1-REQ-WORKSPACE-001` requires workspace context that respects local project instructions.
- `L1-REQ-APP-010` requires persistent configuration for user-scoped and workspace-scoped settings.
- `L2-DES-CONTEXT-001` defines the context assembly step that consumes the assembled instruction content.
- `L2-DES-APP-002` defines configuration source precedence for the settings that control discovery.

## Design Requirement

The program should discover project instruction files by walking the linear directory hierarchy from the project root down to the current working directory, checking each directory for a recognized instruction file in priority order, and assembling the discovered content in root-to-cwd sequence. Global user-level instruction files should be included as a top-level prefix.

Discovery should be deterministic, auditable, bounded, and fast. It must not scan irrelevant directory trees or impose arbitrary depth limits.

## Filename Priority

Each directory is checked for instruction files in this fixed priority order:

| Priority | Filename | Description |
|---|---|---|
| 1 (highest) | `AGENTS.override.md` | User-local override that replaces the standard instruction file in this directory. |
| 2 | `AGENTS.md` | The standard project instruction file. |
| 3+ | User-configured fallbacks | Additional filenames provided through configuration, checked in configuration-specified order. |

Only the first matching regular file found in a given directory is used. If `AGENTS.override.md` exists, `AGENTS.md` and all fallbacks are skipped for that directory. An empty or whitespace-only file is treated as absent — it does not contribute content and does not block lower-priority files in the same directory.

Fallback filenames are not hardcoded to match specific external assistants by name. Instead they are user-configured, which lets projects that already maintain instruction files for other tools (e.g., `CLAUDE.md`, `PROMPT.md`) be recognized without code changes. The default fallback set should cover common external instruction filenames.

## Discovery Algorithm

### Step 1 — Resolve Project Root

Walk upward from the canonicalized current working directory through parent directories. At each ancestor, check for the presence of any configured project-root marker. The default marker set is `[".git"]`.

Stop at the first ancestor that contains any marker. That ancestor is the project root. If no ancestor contains a marker, the project has no discoverable root.

Markers are directory entries, not files. A marker matches when a directory entry with that name exists at that path, regardless of whether it is a file or directory.

The marker set is configurable. An empty marker list disables upward traversal entirely — only the cwd itself is searched.

### Step 2 — Collect Search Path

If a project root was found, collect all directories on the linear path from root to cwd:

```text
cwd → parent → parent → ... → root
```

Reverse this sequence so root comes first:

```text
[root, ..., parent, cwd]
```

If no project root was found, the search path contains only the cwd:

```text
[cwd]
```

### Step 3 — Check Each Directory

For each directory in the search path, for each candidate filename in priority order:

1. Check whether a filesystem entry with that name exists in the directory.
2. If the entry is a regular file and is non-empty after trimming whitespace, select it and stop checking this directory.
3. If the entry is a directory, a special file, empty, or missing, continue to the next candidate filename.
4. If a read error occurs (permission denied, IO error), record a diagnostic for that file and continue checking remaining candidates and remaining directories. A single unreadable file must not abort discovery.
5. Skip entries whose names start with `.` unless they explicitly match a candidate filename (candidates can be hidden files, but arbitrary hidden entries are not candidates).

### Step 4 — Load Global Instructions

Before project-specific instructions, check the user-level configuration directory for global instruction files. On macOS and Linux this is `~/.devo/`; on Windows this is `%USERPROFILE%\.devo\`.

Check in the same priority order as project directories: `AGENTS.override.md`, then `AGENTS.md`. Fallback filenames are not checked at the global level — only the two primary filenames apply.

A missing global directory or missing global instruction files is normal and does not produce a diagnostic.

### Step 5 — Read and Assemble

Read each discovered file's content. The assembled result is:

```text
[Global AGENTS.override.md content, if present]
[Global AGENTS.md content, if present]
[Project root directory instruction file content]
[Intermediate directory instruction file content, in path order]
        ...
[Cwd instruction file content]
```

### Step 6 — Apply Size Bound

The total assembled byte length must not exceed the configured maximum. If it does, truncate from the end of the last file's content and indicate truncation in the assembled output so the model and user understand that content was omitted.

The size check applies after assembly, not per-file. This allows earlier (root-level) files to contribute fully while only the tail of the last file is affected when the total exceeds the bound.

When the maximum is set to zero, all discovery is disabled and the assembled result is empty.

## Global Instructions

Global instruction files in `~/.devo/` apply across all projects and sessions. They are the top-level prefix of the assembled instruction content, appearing before project-root instructions.

Global instructions are discovered on every session start and whenever the assembled instruction content is refreshed. They follow the same filename priority as project directories (`AGENTS.override.md` → `AGENTS.md`), but fallback filenames do not apply at the global level.

Global instructions are subject to the same size bound as project instructions — the total assembled content, including global and project files, must not exceed the configured maximum.

## Configuration

Discovery behavior is controlled through persistent configuration following `L2-DES-APP-002` precedence rules.

| Key | Type | Default | Purpose |
|---|---|---|---|
| `project_doc_max_bytes` | `usize` | `32768` (32 KiB) | Maximum total bytes of assembled instruction content. `0` disables all discovery. |
| `project_doc_fallback_filenames` | `Vec<String>` | sensible default set | Additional filenames to check per directory after `AGENTS.override.md` and `AGENTS.md`. |
| `project_root_markers` | `Vec<String>` | `[".git"]` | Directory entry names that identify project roots during upward traversal. Empty list disables parent traversal. |

Workspace-scoped configuration overrides user-scoped configuration for any of these fields when present. User-scoped fields remain effective when the workspace config omits them. A workspace may specify an empty `project_root_markers` list to scope instruction-file discovery to the cwd only, or a custom marker set to identify roots in non-git workspaces.

## Refresh

Instruction file content must be refreshed when:

- The current working directory changes. Re-discovery must run along the new path before the next model context is assembled.
- A previously discovered instruction file on the active path is modified. The program should detect the change through filesystem watchers, stat polling, or an explicit re-read trigger, and refresh the assembled instructions before the next model invocation.

Refresh should not block session startup. If a refresh is in progress when context assembly begins, the program may use the most recent successfully assembled content and apply the refresh result to subsequent turns.

When a file that was previously absent appears on the path, or a previously present file is deleted, the assembled instructions must reflect the current filesystem state after the next refresh.

## Auditability

The program must make the discovery result understandable to the user. After discovery, the program should expose:

- The canonicalized current working directory.
- The resolved project root, or a statement that no root was found.
- Each discovered file, its directory, and which priority level it matched.
- The total assembled byte count and whether truncation occurred.
- Any diagnostic for unreadable files.

This information should be available through a configuration-inspection or debug view. It should not be emitted as routine model context unless the user requests it.

## Edge Cases

- **No project root found**: Discovery scope is cwd only plus global instructions. No error or warning is produced.
- **Empty marker list**: Parent traversal is disabled. Only cwd is checked (same as no root found, but by user choice rather than because no marker matched).
- **Symlinks**: The canonical path is used for directory comparison so symlink chains are resolved before determining the search path.
- **Filesystem boundaries**: The upward traversal for root discovery stops at the filesystem root. If no marker is found by then, no project root exists.
- **Concurrent modification**: If a file is being written while discovery reads it, the program may read partial content. This is acceptable; the next refresh will correct it.
- **Binary files**: Files that contain null bytes or non-UTF-8 content should be skipped and a diagnostic produced. They are not treated as instruction files.
- **Very large individual files**: A single file that alone exceeds the maximum byte limit is read up to the limit, included as the last (and possibly only) contributing file, and truncation is indicated. Discovery does not skip it just because it is large.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---:|---|---|---|
| refines | L1-REQ-WORKSPACE-002 | 1 | specs/L1/L1-REQ-WORKSPACE-002-project-instruction-files.md | Defines the concrete discovery algorithm, filename resolution, global-instruction support, size bounding, configuration surface, and refresh behavior. |
| related-to | L1-REQ-WORKSPACE-001 | 1 | specs/L1/L1-REQ-WORKSPACE-001-project-context.md | Instruction file discovery provides the local project instructions required by workspace context. |
| related-to | L1-REQ-APP-010 | 1 | specs/L1/L1-REQ-APP-010-configuration.md | Discovery behavior is controlled through persistent configuration with workspace-over-user field precedence. |
| related-to | L2-DES-CONTEXT-001 | 1 | specs/L2/context/L2-DES-CONTEXT-001-context-assembly.md | Assembled instruction content feeds into the instruction hierarchy during context assembly. |
| related-to | L2-DES-APP-002 | 2 | specs/L2/app/L2-DES-APP-002-configuration-precedence.md | Configuration precedence resolves discovery settings from user-scoped and workspace-scoped sources. |
| specified-by | L3-BEH-CORE-008 | 2 | specs/L3/core/L3-BEH-CORE-008-project-instruction-discovery.md | L3 defines project root detection, instruction discovery, global instruction inclusion, size bounding, refresh, and diagnostics. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-22 | Assistant | Initial | Initial project instruction file discovery design covering filename priority, search algorithm, global instructions, configuration, refresh, and auditability. |

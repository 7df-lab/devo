---
artifact_id: L3-BEH-APP-003
revision: 2
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-27
---

# L3-BEH-APP-003 - Traceability Maintenance

## Purpose

Define implementation behavior for repository traceability validation tools. The first required implementation target is `L2 -> L3` validation, but the data model and exit semantics also apply to later `L3 -> implementation` and verification validators.

This document exists because the traceability matrix is not passive documentation. Developers use it to decide whether an L2 design has implementable L3 guidance. A stale matrix row, a missing L3 file, or an embedded `specified-by TBD` row can mislead implementation work as badly as an incorrect behavior spec.

## Source Design

- `L2-DES-TRACE-001` defines matrix schemas, relationship meanings, validation scripts, and CI integration.
- `specs/traceability/l2_to_l3.md` is the central source of truth for L2 to L3 relationships.
- Embedded `Traceability` sections inside spec files are reader-facing projections and must be checked against the central matrix.

## Core Data Contracts

The repository tooling may be implemented in Python, but it must preserve these logical contracts.

```rust
pub enum SpecLevel {
    L1,
    L2,
    L3,
}

pub enum MatrixKind {
    L1ToL2,
    L2ToL3,
    L3ToImpl,
    Verification,
}

pub struct SpecArtifact {
    pub artifact_id: String,
    pub revision: u32,
    pub path: PathBuf,
    pub title: String,
    pub level: SpecLevel,
}

pub struct TraceLink {
    pub source_id: String,
    pub source_path: PathBuf,
    pub target_id: String,
    pub target_path: PathBuf,
    pub relationship: TraceRelationship,
    pub rationale: String,
    pub line_number: usize,
}

pub enum TraceRelationship {
    RefinedBy,
    SpecifiedBy,
    RealizedBy,
    Verifies,
    RelatedTo,
}

pub struct TraceabilityGapReport {
    pub matrix_kind: MatrixKind,
    pub source_total: usize,
    pub primary_linked: Vec<SpecArtifact>,
    pub related_only: Vec<SpecArtifact>,
    pub unlinked: Vec<SpecArtifact>,
    pub stale_sources: Vec<StaleSource>,
    pub stale_targets: Vec<StaleTarget>,
    pub stale_paths: Vec<StalePath>,
    pub duplicate_rows: Vec<DuplicateTraceRow>,
    pub embedded_trace_drifts: Vec<EmbeddedTraceDrift>,
    pub malformed_rows: Vec<MatrixRowDiagnostic>,
}

pub struct MatrixRowDiagnostic {
    pub matrix_path: PathBuf,
    pub line_number: usize,
    pub severity: DiagnosticSeverity,
    pub message: String,
}

pub enum DiagnosticSeverity {
    Warning,
    Error,
}
```

`PathBuf` values in reports are repository-relative paths. Absolute paths may be used internally, but human and JSON output must prefer paths relative to the repository root.

## B1. Resolve Repository Root

- **Trigger**: Any traceability validation script starts.
- **Preconditions**: The script path is known and an optional `--repo` argument may be present.
- **Algorithm / Flow**:
  1. If `--repo` is provided, resolve it relative to the current working directory unless it is already absolute.
  2. If `--repo` is absent, infer the repository root from the script location. For scripts in `specs/`, the default root is the parent of `specs/`.
  3. Verify that the root contains `specs/traceability/`.
  4. Store both the absolute root for file access and the display root for reports.
- **Postconditions**: All scanners use one canonical root.
- **Errors**: Missing root or missing `specs/traceability/` is a usage error and exits with code `2`.

## B2. Parse Spec Artifacts

- **Trigger**: A validator needs source or target artifacts.
- **Preconditions**: The repository root has been resolved.
- **Algorithm / Flow**:
  1. Scan the configured spec directories:
     - L1: `specs/L1/*.md`
     - L2: `specs/L2/**/*.md`
     - L3: `specs/L3/**/*.md`
  2. Exclude archived, template, and generated files only when an explicit ignore rule exists.
  3. Parse YAML frontmatter.
  4. Prefer `artifact_id` from frontmatter. Filename fallback is allowed only to produce a diagnostic; it must not silently define the artifact identity.
  5. Parse `revision` as a positive integer. Missing revision is reported as an artifact diagnostic.
  6. Extract the first `# ` heading as title.
  7. Reject duplicate artifact ids for the same spec level.
- **Postconditions**: The validator has artifact maps keyed by id.
- **Errors**: Duplicate artifact ids are fatal and exit with code `2` because coverage classification would be ambiguous.

## B3. Parse Matrix Files

- **Trigger**: Artifact parsing has completed.
- **Preconditions**: The expected matrix file exists.
- **Algorithm / Flow**:
  1. Read the matrix as UTF-8 Markdown.
  2. Parse table rows that start and end with `|`.
  3. Preserve source line numbers.
  4. Ignore header and separator rows.
  5. Validate the exact column count for the matrix kind.
  6. Validate source and target id prefixes for the matrix kind.
  7. Validate relationship values:
     - `l1_to_l2.md`: `refined-by`, `related-to`
     - `l2_to_l3.md`: `specified-by`, `related-to`
     - `l3_to_impl.md`: `realized-by`
     - `verification.md`: `verifies`
  8. Normalize source and target paths as repository-relative paths.
  9. Keep malformed rows in diagnostics and continue parsing later rows.
- **Postconditions**: The validator has normalized links plus row diagnostics.
- **Errors**: Missing matrix file is fatal and exits with code `2`. Malformed rows are reported as errors in the gap report and make blocking mode exit non-zero.

## B4. Validate L2 To L3 Coverage

- **Trigger**: `specs/l2_l3_traceability_gaps.py` runs.
- **Preconditions**: L2 artifacts, L3 artifacts, and `specs/traceability/l2_to_l3.md` links are parsed.
- **Algorithm / Flow**:
  1. For each L2 artifact, collect matrix rows whose `source_id` matches the artifact id.
  2. Ignore rows whose target L3 artifact is missing when deciding primary coverage; those rows are stale targets.
  3. Classify each L2 artifact:
     - `primary_linked`: at least one valid `specified-by` row to an existing L3 artifact.
     - `related_only`: at least one valid `related-to` row but no valid `specified-by` row.
     - `unlinked`: no valid rows.
  4. Report stale sources when a matrix `source_id` does not exist in L2 artifacts.
  5. Report stale targets when a matrix `target_id` does not exist in L3 artifacts.
  6. Report stale paths when a row id exists but the row path does not match the artifact's current repository-relative path.
  7. Report duplicate rows by the tuple `(source_id, target_id, relationship)`.
  8. Emit text and JSON reports with identical counts.
- **Postconditions**: The output distinguishes real L3 coverage from secondary links and stale rows.
- **Errors**: A stale target must not count as coverage, even if the target id looks valid.

## B5. Validate Embedded Traceability Sections

- **Trigger**: `l2_l3_traceability_gaps.py` runs, or a dedicated embedded-trace validator runs.
- **Preconditions**: The central matrix and L2 file contents are available.
- **Algorithm / Flow**:
  1. Treat `specs/traceability/l2_to_l3.md` as authoritative.
  2. For each L2 artifact, extract rows under the `## Traceability` section.
  3. Compare embedded `specified-by` rows to central matrix `specified-by` rows by target id and target path.
  4. Report `embedded_trace_missing` when the matrix has a `specified-by` row that the L2 file does not show.
  5. Report `embedded_trace_stale_tbd` when the embedded section still has `TBD` for a target where the matrix has concrete L3 coverage.
  6. Report `embedded_trace_extra` when the embedded section names a `specified-by` target that the central matrix does not contain.
  7. Report `embedded_trace_revision_drift` when the embedded target revision disagrees with the target L3 frontmatter revision.
  8. Do not modify files automatically in validation mode.
- **Postconditions**: Developers can detect drift between the central matrix and reader-facing trace sections.
- **Errors**: Embedded drift is a reportable gap. Blocking behavior is controlled by exit mode.

## B6. Validate L3 To Implementation And Verification Matrices

- **Trigger**: `l3_impl_traceability_gaps.py` or `verification_gaps.py` runs.
- **Preconditions**: L3 artifacts and implementation/test metadata are available.
- **Algorithm / Flow**:
  1. Parse `l3_to_impl.md` and verify that every `Spec ID` points to an existing L3 artifact.
  2. Verify each implementation path exists when the implementation matrix is intended to be active.
  3. Parse Rust test trace comments with the exact `Trace:` and `Verifies:` format from `L2-DES-TRACE-001`.
  4. Cross-check `verification.md` rows against discovered test trace comments.
  5. Report tests with trace comments but no verification row.
  6. Report verification rows whose test path or target spec no longer exists.
- **Postconditions**: Implementation and verification links can be audited without trusting manual matrix rows alone.
- **Errors**: Missing implementation and verification links may be advisory until the project declares an implementation baseline.

## B7. Output Schema

Text output must include:

- Repository display path.
- Matrix path.
- Source artifact total.
- Primary linked count.
- Related-only count.
- Unlinked count.
- Stale source count.
- Stale target count.
- Stale path count.
- Duplicate row count.
- Embedded drift count when checked.

JSON output must include:

```json
{
  "repo": ".",
  "matrix_kind": "l2_to_l3",
  "traceability_path": "specs/traceability/l2_to_l3.md",
  "counts": {
    "source_total": 0,
    "primary_linked": 0,
    "related_only": 0,
    "unlinked": 0,
    "stale_sources": 0,
    "stale_targets": 0,
    "stale_paths": 0,
    "duplicate_rows": 0,
    "embedded_trace_drifts": 0,
    "malformed_rows": 0
  },
  "unlinked": [],
  "related_only": [],
  "stale_sources": [],
  "stale_targets": [],
  "stale_paths": [],
  "duplicate_rows": [],
  "embedded_trace_drifts": [],
  "malformed_rows": []
}
```

JSON field names are stable API for CI and dashboards. Adding fields is allowed; renaming existing fields requires a revision bump and migration note.

## B8. Exit Semantics

Validators support two modes:

- `blocking`: default for direct script execution.
- `advisory`: selected by `--advisory` for baseline discovery and non-blocking CI.

Blocking mode exits:

| Exit Code | Meaning |
|---:|---|
| 0 | No primary coverage gaps, stale links, stale paths, duplicate rows, malformed rows, or embedded drifts. |
| 1 | Validation completed and found traceability gaps or drift. |
| 2 | Usage error, missing required matrix, missing required directory, unreadable required file, or duplicate artifact ids. |

Advisory mode must preserve the same report content but exit `0` when validation completed.

## B9. Required Scripts

Required now:

- `specs/l1_l2_traceability_gaps.py`
- `specs/l2_l3_traceability_gaps.py`

Required before implementation tracking is treated as complete:

- `specs/l3_impl_traceability_gaps.py`
- `specs/verification_gaps.py`

Shared parsing utilities may be extracted after the second script would otherwise duplicate matrix parsing, artifact parsing, and report formatting. The shared module must remain local to `specs/` and must not access the network.

## B10. Required Tests

Traceability tooling needs fixture-based tests covering:

- Duplicate artifact ids are fatal.
- Missing artifact id in frontmatter is reported.
- L2 artifact with no row is `unlinked`.
- L2 artifact with only `related-to` rows is `related_only`.
- `specified-by` row with a missing L3 target is `stale_targets` and does not count as coverage.
- Matrix source id with no L2 artifact is `stale_sources`.
- Matrix source or target path mismatch is `stale_paths`.
- Duplicate source-target-relationship rows are reported.
- Malformed matrix rows include line numbers.
- Embedded `specified-by TBD` is reported when the matrix has a concrete L3 target.
- Embedded target revision drift is reported when the L3 frontmatter revision changes.
- JSON and text outputs agree on counts.
- Advisory mode returns `0` while preserving the gap report.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| specifies | L2-DES-TRACE-001 | 1 | specs/L2/traceability/L2-DES-TRACE-001-traceability-system.md | Implements matrix parsing, L2-L3 gap detection, stale link detection, embedded trace drift checks, and validation exit semantics. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-27 | Assistant | Initial | Initial L3 traceability maintenance behavior. |
| 2 | 2026-05-27 | Assistant | Correction | Restored the missing L3 artifact and made validation behavior concrete enough to guide L2-L3 tooling, embedded trace checks, output schemas, exit codes, and tests. |

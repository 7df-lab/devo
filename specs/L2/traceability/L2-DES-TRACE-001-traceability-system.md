---
artifact_id: L2-DES-TRACE-001
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-26
---

# L2-DES-TRACE-001 — Traceability System Design

## Purpose

Define the technical design of the specification traceability system: the matrix files, relationship types, validation tools, and integration points that maintain the `L1 → L2 → L3 → Implementation → Tests` chain. This design explains how traceability is structured, maintained, verified, and consumed by both humans and tools.

## Background / Context

The project follows specification-driven development with an explicit L1/L2/L3 hierarchy. Traceability is the mechanism that links requirements to designs, designs to behavior specs, behavior specs to code, and code to tests. Without a well-designed traceability system, the specification hierarchy becomes documentation that drifts from implementation.

The traceability system must support:

- Coverage analysis: which L1 requirements lack L2 refinement, which L2 designs lack L3 detail, etc.
- Impact analysis: if an L1 requirement changes, which L2/L3/impl artifacts are affected.
- Verification auditing: which tests verify which specifications.
- Gap detection: automated tooling to find missing links.

## Architecture Overview

The traceability system has four components:

1. **Matrix files** — Markdown tables under `specs/traceability/` that are the single source of truth for all traceability relationships.
2. **Validation scripts** — Python scripts that scan specs and matrices to detect gaps, stale links, and coverage issues.
3. **Test trace comments** — Structured comments in Rust source that declare what each test verifies.
4. **Workflow integration** — Conventions for when and how to update traceability links during development.

### Component Diagram

```
specs/
  L1/                           L1 requirements (input)
  L2/                           L2 designs (input)
  L3/                           L3 behavior specs (input)
  traceability/
    l1_to_l2.md                 Matrix: L1 → L2
    l2_to_l3.md                 Matrix: L2 → L3
    l3_to_impl.md               Matrix: L3 → implementation
    verification.md             Matrix: tests → specs
  l1_l2_traceability_gaps.py    Validation tool

crates/
  */src/**/*.rs                 Implementation with trace comments
```

## Matrix File Schema

### l1_to_l2.md

Maps L1 requirements to the L2 designs that refine them.

| Column | Type | Description |
|---|---|---|
| Source ID | `L1-REQ-<DOMAIN>-<NNN>` | The L1 requirement artifact identifier |
| Source Path | Relative path | Path to the L1 spec file |
| Target ID | `L2-DES-<DOMAIN>-<NNN>` | The L2 design artifact identifier |
| Target Path | Relative path | Path to the L2 spec file |
| Relationship | `refined-by` or `related-to` | `refined-by` is the primary relationship; `related-to` is a secondary link |
| Rationale | Text | Explanation of why this link exists |

### l2_to_l3.md

Maps L2 designs to L3 behavior specifications.

| Column | Type | Description |
|---|---|---|
| Source ID | `L2-DES-<DOMAIN>-<NNN>` | The L2 design artifact identifier |
| Source Path | Relative path | Path to the L2 spec file |
| Target ID | `L3-BEH-<DOMAIN>-<NNN>` | The L3 behavior spec artifact identifier |
| Target Path | Relative path | Path to the L3 spec file |
| Relationship | `specified-by` or `related-to` | `specified-by` is the primary relationship |
| Rationale | Text | Explanation of why this link exists |

### l3_to_impl.md

Maps L3 behavior specifications to implementation files and symbols.

| Column | Type | Description |
|---|---|---|
| Spec ID | `L3-BEH-<DOMAIN>-<NNN>` | The L3 artifact identifier |
| Revision | Integer | The revision of the L3 spec being implemented |
| Spec Path | Relative path | Path to the L3 spec file |
| Implementation Path | Relative path | Path to the source file |
| Symbol / Module | Rust path | The module, struct, function, or trait that realizes the spec |
| Relationship | `realized-by` | Always `realized-by` |
| Notes | Text | Optional implementation context |

### verification.md

Maps tests to the specification artifacts they verify.

| Column | Type | Description |
|---|---|---|
| Test Reference | Rust path | Fully qualified test function path |
| Test Type | `Unit`, `Integration`, or `End-to-End` | The test level |
| Test Location | Relative path | Path to the source file containing the test |
| Directly Verifies | `L3-BEH-<DOMAIN>-<NNN>` | The spec artifact directly verified |
| Verified Revision | Integer | The revision of the verified spec |
| Derived Coverage | Comma-separated L2/L1 IDs | L2 and L1 coverage derived through the traceability chain |
| Notes | Text | Description of what behavior is verified |

## Relationship Types

### Primary Relationships

These form the forward traceability chain. Every L1 requirement should have at least one `refined-by` link to an L2 design. Every L2 design should have at least one `specified-by` link to an L3 spec. Every L3 spec targeted for implementation should have at least one `realized-by` link. Every implemented spec should have at least one test `verifies` link.

| Relationship | Direction | Semantics |
|---|---|---|
| `refined-by` | L1 → L2 | The L2 design is the primary technical refinement of the L1 requirement. |
| `specified-by` | L2 → L3 | The L3 behavior spec provides the concrete, implementable detail required by the L2 design. |
| `realized-by` | L3 → Implementation | The source file or symbol implements the behavior defined by the L3 spec. |
| `verifies` | Test → Spec | The test exercises and confirms the behavior defined by the spec. |

### Secondary Relationships

`related-to` captures meaningful but non-primary connections. For example, a safety requirement may be `related-to` the client-server protocol because protocol events carry safety decisions, even though the primary refinement is the safety subsystem design.

Secondary links are not used for primary coverage metrics but are valuable for impact analysis and design review.

## Gap Detection

### l1_l2_traceability_gaps.py

The existing gap detection script (`specs/l1_l2_traceability_gaps.py`) scans L1 specs and compares them against the `l1_to_l2.md` matrix, then reports:

- **No L2 link**: L1 requirements with zero entries in the matrix.
- **Linked but not refined-by**: L1 requirements that have `related-to` links but no `refined-by` link.
- **Stale trace sources**: Matrix rows referencing L1 IDs that no longer exist.

The script supports both human-readable output and machine-readable JSON (`--json` flag) for CI integration.

### Design for Future Gap Scripts

Future gap detection scripts for `l2_to_l3`, `l3_to_impl`, and `verification` should follow the same pattern:

1. Scan source artifacts (L2, L3, or implementation files) by ID.
2. Scan the corresponding matrix file for existing links.
3. Report gaps by category: unlinked, linked-but-not-primary, stale links.
4. Support both human-readable and JSON output.
5. Exit with a non-zero status when gaps are found (for CI gating).

These scripts should live alongside `l1_l2_traceability_gaps.py` in `specs/` and share common parsing utilities.

## Validation Strategy

### At Spec Change Time

When a new spec artifact is created or an existing one is modified:

1. If it is a new artifact, add a row to the appropriate matrix file.
2. If it changes the artifact's scope, review existing trace links for accuracy.
3. If it supersedes a previous revision, add the new revision row without removing the old baseline row.

### At Implementation Time

When code is written against a spec:

1. Add an `l3_to_impl` row linking the L3 spec to the implementation file and symbol.
2. Add test trace comments (`/// Trace:`, `/// Verifies:`) above the test functions.
3. Add a `verification.md` row linking the test to the spec.

### In CI

Gap detection scripts can run as CI checks:

```bash
python3 specs/l1_l2_traceability_gaps.py --json | \
  python3 -c "import sys,json; d=json.load(sys.stdin); sys.exit(1 if d['counts']['unlinked'] > 0 else 0)"
```

This gates merges on traceability completeness. The CI check should be advisory initially, becoming a blocking gate once the team establishes baseline coverage.

## Test Trace Comment Format

Tests must declare traceability using structured comments immediately above the test function:

```rust
/// Trace: L3-BEH-DOMAIN-NNN
/// Verifies: <short description of the behavior being verified>
#[test]
fn test_name() {
    // ...
}
```

Design decisions:

- The `Trace` line is mandatory for tests that verify behavior derived from specs.
- The `Verifies` line is recommended for human readability.
- Multiple spec IDs can be listed comma-separated when a test spans multiple specs.
- The comment uses Rust doc-comment syntax (`///`) so it is visible to `rustdoc` and IDE hover.
- The format is deliberately simple (not a custom attribute or macro) to minimize tooling friction.

## Coverage Model

Coverage is computed transitively through the traceability chain:

```
Test → (verifies) → L3 → (specified-by) → L2 → (refined-by) → L1
```

A unit test for an L3 behavior contributes to L1 and L2 coverage through the traceability matrix, without requiring direct test-to-L1 or test-to-L2 links. This avoids an N×M explosion of trace rows.

The recommended mapping:
- **Unit Test** → L3 (direct), L2 and L1 (derived)
- **Integration Test** → L3 and/or L2 (direct)
- **End-to-End Test** → L1 and/or L2 (direct)

### Coverage Completeness Rules

A specification level is "fully covered" when:

| Level | Condition |
|---|---|
| L1 | Every L1 requirement has at least one `refined-by` link to an L2 design. |
| L2 | Every L2 design has at least one `specified-by` link to an L3 spec. |
| L3 | Every L3 spec has at least one `realized-by` link AND at least one `verifies` link from a test. |

## Integration with Development Workflow

The preferred workflow when implementing a behavior:

1. Identify the relevant L1 / L2 / L3 artifacts.
2. Select or create the L3 artifact that defines the concrete behavior.
3. Implement the behavior in the corresponding source file.
4. Add or update the `l3_to_impl.md` row.
5. Add or update tests with trace comments.
6. Add or update the `verification.md` row.
7. Run gap detection to confirm no new gaps were introduced.

## File Organization

Matrix files are co-located under `specs/traceability/` rather than distributed across domain directories. This centralization ensures:

- A single source of truth for all cross-artifact relationships.
- Gap detection tools can operate on one file per relationship type.
- Impact analysis can traverse the full chain without directory-hopping.
- The traceability state can be reviewed in one place.

The tradeoff is that individual spec files do not self-document their own trace links. This is mitigated by the `Traceability` section in the L1 template and by the fact that the matrix is Markdown and human-readable.

## Tooling Architecture

### Existing

| Tool | Location | Purpose |
|---|---|---|
| `l1_l2_traceability_gaps.py` | `specs/` | Scan L1 specs and report gaps in `l1_to_l2.md` |

### Planned

| Tool | Purpose |
|---|---|
| `l2_l3_traceability_gaps.py` | Scan L2 specs and report gaps in `l2_to_l3.md` |
| `l3_impl_traceability_gaps.py` | Scan L3 specs against `l3_to_impl.md` and source files with trace comments |
| `verification_gaps.py` | Cross-reference test trace comments with `verification.md` |

### Shared Library

As the number of scripts grows, extract common functionality into a shared Python module under `specs/`:

- `spec_parser.py` — Parse spec frontmatter, extract artifact IDs, titles, and revisions.
- `matrix_parser.py` — Parse traceability matrix Markdown tables.
- `reporter.py` — Format human-readable and JSON gap reports.

## Design Decisions

1. **Markdown tables over structured data formats (YAML, JSON).** Markdown is human-readable in diffs and GitHub renders it natively. The table format is simple enough for reliable machine parsing.

2. **Centralized matrices over distributed links.** Keeping all links in one file per relationship type makes gap detection and impact analysis straightforward. Distributed links (e.g., back-references in each spec) would make coverage queries expensive.

3. **Python scripts over Rust tooling.** Traceability validation is a development-time concern, not a runtime concern. Python scripts are easier to write, modify, and integrate into CI than compiled Rust binaries.

4. **Transitive coverage over direct test-to-L1 links.** Requiring every test to declare derived coverage manually would create maintenance burden. Transitive derivation through the matrix keeps the verification table focused on direct L3 verification.

5. **`refined-by` as primary, `related-to` as secondary.** Not every L1-L2 connection is a direct refinement. Separating primary from secondary links allows accurate coverage metrics while preserving valuable cross-references for impact analysis.

## Error Handling and Edge Cases

- **Duplicate artifact IDs**: The gap detection script must reject duplicate IDs and fail loudly rather than silently ignoring duplicates.
- **Stale links**: When a spec is renamed or removed, the matrix may contain orphan rows. Gap scripts detect these and report them.
- **Missing matrix files**: Gap scripts should report a clear error if an expected matrix file does not exist, rather than silently reporting zero links.
- **Malformed table rows**: Rows that do not parse as valid matrix entries should be reported as warnings but should not block the rest of the analysis.
- **Circular traceability**: L1 → L2 → L3 should be acyclic. While the current tooling does not detect cycles, the relationship direction is defined to prevent them.

## Concurrency and Collaboration

Traceability matrices are plain text files. Merge conflicts are expected when multiple branches add rows to the same matrix file. To minimize conflicts:

- Each new link is a single row appended to the table.
- Rows within a matrix file can be sorted by Source ID for deterministic ordering.
- A CI check can reject merges that introduce duplicate rows.

## Observability

The traceability system itself should be observable:

- Gap detection script output is the primary observability channel.
- JSON output enables dashboarding of coverage trends over time.
- CI integration surfaces gaps before they reach the main branch.

## Security and Edge Cases

- Traceability files are documentation, not runtime configuration. They must not contain secrets, credentials, or sensitive paths.
- The gap detection scripts are development tools. They must not access network resources, write to files outside the repo, or execute untrusted input.
- Stale trace links to deprecated specs are intentional and should not be automatically removed — they preserve historical context.

## Testing Strategy

Tests for the traceability system itself:

- Unit tests for the ID regex pattern against valid and invalid artifact IDs.
- Unit tests for Markdown table cell parsing against edge cases (empty cells, escaped pipes, multi-line cells).
- Integration tests for the gap detection script against a fixture `specs/` tree with known gaps.
- The gap detection script should return exit code 0 when no gaps exist and non-zero when gaps are found.

## Dependencies With Other Specification Artifacts

- `specs/AGENTS.md` defines the methodology that the traceability system implements.
- `specs/templates/spec-l1-requirement.md` defines the L1 frontmatter that gap scripts parse.
- Individual L1/L2/L3 specs are the source artifacts that matrices reference.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---:|---|---|---|
| specified-by | L3-BEH-APP-003 | 2 | specs/L3/app/L3-BEH-APP-003-traceability-maintenance.md | L3 defines matrix parsing, L2-L3 gap detection, stale link detection, embedded trace drift checks, and validation exit semantics. |

## Open Questions

- Whether `l3_to_impl.md` should be auto-generated from test trace comments instead of manually maintained.
- Whether gap detection should run as a pre-commit hook, a CI check, or both.
- Whether the traceability system should support "partial coverage" annotations (e.g., an L3 spec that is 80% implemented).

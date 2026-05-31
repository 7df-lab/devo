#!/usr/bin/env python3
"""Report L2 designs that are not specified by L3 behavior specs."""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from dataclasses import dataclass
from pathlib import Path


DEFAULT_REPO = Path(__file__).parent.parent
TRACEABILITY_PATH = Path("specs") / "traceability" / "l2_to_l3.md"


class UsageError(Exception):
    pass


@dataclass(frozen=True)
class SpecArtifact:
    artifact_id: str
    revision: int | None
    path: Path
    title: str
    level: str


@dataclass(frozen=True)
class MarkdownCell:
    value: str
    line_number: int


@dataclass(frozen=True)
class TraceLink:
    source_id: str
    source_path: str
    target_id: str
    target_path: str
    relationship: str
    rationale: str
    line_number: int


@dataclass(frozen=True)
class MatrixRowDiagnostic:
    matrix_path: str
    line_number: int
    severity: str
    message: str


@dataclass(frozen=True)
class DuplicateTraceRow:
    source_id: str
    target_id: str
    relationship: str
    line_numbers: list[int]


@dataclass(frozen=True)
class EmbeddedTraceDrift:
    source_id: str
    kind: str
    target_id: str
    line_number: int
    message: str


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Find L2 designs missing L3 traceability links."
    )
    parser.add_argument(
        "--repo",
        type=Path,
        default=DEFAULT_REPO,
        help="Repository root to scan. Default: parent of this script directory.",
    )
    parser.add_argument("--json", action="store_true", help="Emit JSON.")
    parser.add_argument(
        "--advisory",
        action="store_true",
        help="Exit 0 after successful validation even when gaps are found.",
    )
    return parser.parse_args()


def resolve_repo(path: Path) -> Path:
    expanded = path.expanduser()
    if expanded.is_absolute():
        repo = expanded.resolve()
    else:
        repo = (Path.cwd() / expanded).resolve()
    if not (repo / "specs" / "traceability").is_dir():
        raise UsageError(f"Missing specs/traceability under repository root: {repo}")
    return repo


def display_path(path: Path) -> str:
    try:
        relative = os.path.relpath(path, start=Path.cwd())
    except ValueError:
        return str(path)
    return "." if relative == "." else relative


def read_text(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except FileNotFoundError as exc:
        raise UsageError(f"Missing required path: {path}") from exc


def markdown_cells(line: str, line_number: int) -> list[MarkdownCell]:
    stripped = line.strip()
    if not stripped.startswith("|") or not stripped.endswith("|"):
        return []

    cells: list[str] = []
    current: list[str] = []
    escaped = False
    for char in stripped[1:-1]:
        if escaped:
            current.append(char)
            escaped = False
            continue
        if char == "\\":
            escaped = True
            continue
        if char == "|":
            cells.append("".join(current).strip())
            current = []
            continue
        current.append(char)
    cells.append("".join(current).strip())
    return [MarkdownCell(value=cell, line_number=line_number) for cell in cells]


def extract_frontmatter(text: str) -> dict[str, str]:
    if not text.startswith("---\n"):
        return {}
    end = text.find("\n---", 4)
    if end == -1:
        return {}
    fields: dict[str, str] = {}
    for line in text[4:end].splitlines():
        if ":" not in line:
            continue
        key, value = line.split(":", 1)
        fields[key.strip()] = value.strip()
    return fields


def extract_title(text: str, fallback: str) -> str:
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("# "):
            return stripped.lstrip("#").strip()
    return fallback


def parse_revision(value: str | None) -> int | None:
    if value is None:
        return None
    try:
        revision = int(value)
    except ValueError:
        return None
    if revision <= 0:
        return None
    return revision


def collect_spec_artifacts(repo: Path, level: str) -> tuple[dict[str, SpecArtifact], list[MatrixRowDiagnostic]]:
    spec_dir = repo / "specs" / level
    if not spec_dir.is_dir():
        raise UsageError(f"Missing {level} directory: {spec_dir}")

    artifacts: dict[str, SpecArtifact] = {}
    diagnostics: list[MatrixRowDiagnostic] = []
    duplicate_ids: dict[str, list[Path]] = {}

    for path in sorted(spec_dir.rglob("*.md")):
        text = read_text(path)
        frontmatter = extract_frontmatter(text)
        artifact_id = frontmatter.get("artifact_id")
        relative_path = path.relative_to(repo)
        if artifact_id is None:
            diagnostics.append(
                MatrixRowDiagnostic(
                    matrix_path=str(relative_path),
                    line_number=1,
                    severity="Warning",
                    message="missing artifact_id frontmatter",
                )
            )
            artifact_id = path.stem

        if artifact_id in artifacts:
            duplicate_ids.setdefault(artifact_id, [artifacts[artifact_id].path]).append(
                relative_path
            )
            continue

        artifacts[artifact_id] = SpecArtifact(
            artifact_id=artifact_id,
            revision=parse_revision(frontmatter.get("revision")),
            path=relative_path,
            title=extract_title(text, path.stem),
            level=level,
        )

    if duplicate_ids:
        details = "\n".join(
            f"  {artifact_id}: {', '.join(str(path) for path in paths)}"
            for artifact_id, paths in duplicate_ids.items()
        )
        raise UsageError(f"Duplicate {level} artifact ids found:\n{details}")

    return artifacts, diagnostics


def is_separator_row(cells: list[MarkdownCell]) -> bool:
    return all(set(cell.value) <= {"-", ":"} for cell in cells)


def parse_trace_links(repo: Path) -> tuple[list[TraceLink], list[MatrixRowDiagnostic]]:
    matrix_path = repo / TRACEABILITY_PATH
    text = read_text(matrix_path)
    links: list[TraceLink] = []
    diagnostics: list[MatrixRowDiagnostic] = []
    in_matrix_table = False

    for line_number, line in enumerate(text.splitlines(), start=1):
        if in_matrix_table and line.startswith("## "):
            break
        cells = markdown_cells(line, line_number)
        if not cells:
            continue
        if cells[0].value == "Source ID":
            in_matrix_table = True
            continue
        if not in_matrix_table or is_separator_row(cells):
            continue
        if len(cells) != 6:
            diagnostics.append(
                MatrixRowDiagnostic(
                    matrix_path=str(TRACEABILITY_PATH),
                    line_number=line_number,
                    severity="Error",
                    message=f"expected 6 columns, found {len(cells)}",
                )
            )
            continue

        source_id, source_path, target_id, target_path, relationship, rationale = [
            cell.value for cell in cells
        ]
        row_errors: list[str] = []
        if not source_id.startswith("L2-"):
            row_errors.append("source id must start with L2-")
        if not target_id.startswith("L3-"):
            row_errors.append("target id must start with L3-")
        if relationship not in {"specified-by", "related-to"}:
            row_errors.append("relationship must be specified-by or related-to")
        if row_errors:
            diagnostics.append(
                MatrixRowDiagnostic(
                    matrix_path=str(TRACEABILITY_PATH),
                    line_number=line_number,
                    severity="Error",
                    message="; ".join(row_errors),
                )
            )
            continue

        links.append(
            TraceLink(
                source_id=source_id,
                source_path=normalize_repo_path(source_path),
                target_id=target_id,
                target_path=normalize_repo_path(target_path),
                relationship=relationship,
                rationale=rationale,
                line_number=line_number,
            )
        )

    return links, diagnostics


def normalize_repo_path(path: str) -> str:
    return str(Path(path))


def artifact_to_dict(item: SpecArtifact) -> dict[str, str | int | None]:
    return {
        "artifact_id": item.artifact_id,
        "revision": item.revision,
        "path": str(item.path),
        "title": item.title,
    }


def link_to_dict(link: TraceLink) -> dict[str, str | int]:
    return {
        "source_id": link.source_id,
        "source_path": link.source_path,
        "target_id": link.target_id,
        "target_path": link.target_path,
        "relationship": link.relationship,
        "rationale": link.rationale,
        "line_number": link.line_number,
    }


def diagnostic_to_dict(diagnostic: MatrixRowDiagnostic) -> dict[str, str | int]:
    return {
        "matrix_path": diagnostic.matrix_path,
        "line_number": diagnostic.line_number,
        "severity": diagnostic.severity,
        "message": diagnostic.message,
    }


def duplicate_to_dict(duplicate: DuplicateTraceRow) -> dict[str, str | list[int]]:
    return {
        "source_id": duplicate.source_id,
        "target_id": duplicate.target_id,
        "relationship": duplicate.relationship,
        "line_numbers": duplicate.line_numbers,
    }


def drift_to_dict(drift: EmbeddedTraceDrift) -> dict[str, str | int]:
    return {
        "source_id": drift.source_id,
        "kind": drift.kind,
        "target_id": drift.target_id,
        "line_number": drift.line_number,
        "message": drift.message,
    }


def collect_duplicates(links: list[TraceLink]) -> list[DuplicateTraceRow]:
    by_key: dict[tuple[str, str, str], list[int]] = {}
    for link in links:
        key = (link.source_id, link.target_id, link.relationship)
        by_key.setdefault(key, []).append(link.line_number)
    return [
        DuplicateTraceRow(
            source_id=source_id,
            target_id=target_id,
            relationship=relationship,
            line_numbers=line_numbers,
        )
        for (source_id, target_id, relationship), line_numbers in sorted(by_key.items())
        if len(line_numbers) > 1
    ]


def collect_stale_paths(
    links: list[TraceLink],
    l2: dict[str, SpecArtifact],
    l3: dict[str, SpecArtifact],
) -> list[dict[str, str | int]]:
    stale_paths: list[dict[str, str | int]] = []
    for link in links:
        source = l2.get(link.source_id)
        target = l3.get(link.target_id)
        if source is not None and link.source_path != str(source.path):
            stale_paths.append(
                {
                    "id": link.source_id,
                    "expected_path": str(source.path),
                    "matrix_path": link.source_path,
                    "line_number": link.line_number,
                }
            )
        if target is not None and link.target_path != str(target.path):
            stale_paths.append(
                {
                    "id": link.target_id,
                    "expected_path": str(target.path),
                    "matrix_path": link.target_path,
                    "line_number": link.line_number,
                }
            )
    return stale_paths


def parse_embedded_traceability(text: str) -> list[tuple[int, list[str]]]:
    lines = text.splitlines()
    in_section = False
    rows: list[tuple[int, list[str]]] = []
    for line_number, line in enumerate(lines, start=1):
        stripped = line.strip()
        if stripped.startswith("## "):
            in_section = stripped == "## Traceability"
            continue
        if not in_section:
            continue
        cells = markdown_cells(line, line_number)
        if not cells:
            continue
        if cells[0].value == "Relationship" or is_separator_row(cells):
            continue
        rows.append((line_number, [cell.value for cell in cells]))
    return rows


def collect_embedded_drifts(
    repo: Path,
    l2: dict[str, SpecArtifact],
    l3: dict[str, SpecArtifact],
    links: list[TraceLink],
) -> list[EmbeddedTraceDrift]:
    matrix_by_source: dict[str, dict[str, TraceLink]] = {}
    for link in links:
        if link.relationship == "specified-by" and link.target_id in l3:
            matrix_by_source.setdefault(link.source_id, {})[link.target_id] = link
    drifts: list[EmbeddedTraceDrift] = []

    for source_id, source in sorted(l2.items()):
        matrix_links = matrix_by_source.get(source_id, {})
        if not matrix_links:
            continue
        text = read_text(repo / source.path)
        embedded_targets: dict[str, int] = {}
        for line_number, cells in parse_embedded_traceability(text):
            if len(cells) < 4:
                continue
            relationship, target_id, target_revision, target_path = cells[:4]
            if relationship != "specified-by":
                continue
            if target_id == "TBD":
                first_matrix_target = next(iter(matrix_links))
                drifts.append(
                    EmbeddedTraceDrift(
                        source_id=source_id,
                        kind="embedded_trace_stale_tbd",
                        target_id=first_matrix_target,
                        line_number=line_number,
                        message="embedded traceability still contains TBD while matrix has specified-by coverage",
                    )
                )
                continue
            embedded_targets[target_id] = line_number
            if target_id not in matrix_links:
                drifts.append(
                    EmbeddedTraceDrift(
                        source_id=source_id,
                        kind="embedded_trace_extra",
                        target_id=target_id,
                        line_number=line_number,
                        message="embedded specified-by target is not present in central matrix",
                    )
                )
                continue
            target = l3.get(target_id)
            if target is None:
                continue
            if target_path != str(target.path):
                drifts.append(
                    EmbeddedTraceDrift(
                        source_id=source_id,
                        kind="embedded_trace_extra",
                        target_id=target_id,
                        line_number=line_number,
                        message=f"embedded target path {target_path} does not match {target.path}",
                    )
                )
            if str(target.revision) != target_revision:
                drifts.append(
                    EmbeddedTraceDrift(
                        source_id=source_id,
                        kind="embedded_trace_revision_drift",
                        target_id=target_id,
                        line_number=line_number,
                        message=f"embedded revision {target_revision} does not match L3 revision {target.revision}",
                    )
                )

        for target_id, link in sorted(matrix_links.items()):
            if target_id not in embedded_targets:
                drifts.append(
                    EmbeddedTraceDrift(
                        source_id=source_id,
                        kind="embedded_trace_missing",
                        target_id=target_id,
                        line_number=link.line_number,
                        message="central matrix specified-by target is missing from embedded traceability",
                    )
                )

    return drifts


def build_report(repo: Path) -> dict[str, object]:
    l2, l2_diagnostics = collect_spec_artifacts(repo, "L2")
    l3, l3_diagnostics = collect_spec_artifacts(repo, "L3")
    links, row_diagnostics = parse_trace_links(repo)
    duplicates = collect_duplicates(links)
    stale_sources = sorted({link.source_id for link in links if link.source_id not in l2})
    stale_targets = sorted({link.target_id for link in links if link.target_id not in l3})
    stale_paths = collect_stale_paths(links, l2, l3)
    embedded_drifts = collect_embedded_drifts(repo, l2, l3, links)

    links_by_source: dict[str, list[TraceLink]] = {}
    for link in links:
        links_by_source.setdefault(link.source_id, []).append(link)

    primary_linked: list[SpecArtifact] = []
    related_only: list[SpecArtifact] = []
    unlinked: list[SpecArtifact] = []
    for artifact_id, artifact in sorted(l2.items()):
        valid_links = [
            link
            for link in links_by_source.get(artifact_id, [])
            if link.target_id in l3
        ]
        if any(link.relationship == "specified-by" for link in valid_links):
            primary_linked.append(artifact)
        elif valid_links:
            related_only.append(artifact)
        else:
            unlinked.append(artifact)

    malformed_rows = [*l2_diagnostics, *l3_diagnostics, *row_diagnostics]
    counts = {
        "source_total": len(l2),
        "primary_linked": len(primary_linked),
        "related_only": len(related_only),
        "unlinked": len(unlinked),
        "stale_sources": len(stale_sources),
        "stale_targets": len(stale_targets),
        "stale_paths": len(stale_paths),
        "duplicate_rows": len(duplicates),
        "embedded_trace_drifts": len(embedded_drifts),
        "malformed_rows": len(malformed_rows),
    }

    return {
        "repo": display_path(repo),
        "matrix_kind": "l2_to_l3",
        "traceability_path": str(TRACEABILITY_PATH),
        "counts": counts,
        "primary_linked": [artifact_to_dict(item) for item in primary_linked],
        "related_only": [artifact_to_dict(item) for item in related_only],
        "unlinked": [artifact_to_dict(item) for item in unlinked],
        "stale_sources": stale_sources,
        "stale_targets": stale_targets,
        "stale_paths": stale_paths,
        "duplicate_rows": [duplicate_to_dict(item) for item in duplicates],
        "embedded_trace_drifts": [drift_to_dict(item) for item in embedded_drifts],
        "malformed_rows": [diagnostic_to_dict(item) for item in malformed_rows],
    }


def has_gaps(report: dict[str, object]) -> bool:
    counts = report["counts"]
    assert isinstance(counts, dict)
    return any(
        int(counts[key]) > 0
        for key in [
            "related_only",
            "unlinked",
            "stale_sources",
            "stale_targets",
            "stale_paths",
            "duplicate_rows",
            "embedded_trace_drifts",
            "malformed_rows",
        ]
    )


def print_text_report(report: dict[str, object]) -> None:
    counts = report["counts"]
    assert isinstance(counts, dict)
    print("L2 to L3 Traceability Gap Report")
    print(f"Repository: {report['repo']}")
    print(f"Traceability matrix: {report['traceability_path']}")
    print()
    print(f"L2 total: {counts['source_total']}")
    print(f"Specified by at least one L3 item: {counts['primary_linked']}")
    print(f"Linked to L3 but not specified-by: {counts['related_only']}")
    print(f"No L3 link: {counts['unlinked']}")
    print(f"Stale sources: {counts['stale_sources']}")
    print(f"Stale targets: {counts['stale_targets']}")
    print(f"Stale paths: {counts['stale_paths']}")
    print(f"Duplicate rows: {counts['duplicate_rows']}")
    print(f"Embedded trace drifts: {counts['embedded_trace_drifts']}")
    print(f"Malformed rows: {counts['malformed_rows']}")

    for title, key in [
        ("No L3 link", "unlinked"),
        ("Linked to L3 but not specified-by", "related_only"),
        ("Malformed rows", "malformed_rows"),
    ]:
        items = report[key]
        assert isinstance(items, list)
        print(f"\n{title} ({len(items)})")
        print("-" * len(f"{title} ({len(items)})"))
        if not items:
            print("None")
            continue
        for item in items:
            print(f"- {item}")


def main() -> int:
    args = parse_args()
    try:
        repo = resolve_repo(args.repo)
        report = build_report(repo)
    except UsageError as exc:
        print(str(exc), file=sys.stderr)
        return 2

    if args.json:
        print(json.dumps(report, indent=2, sort_keys=True))
    else:
        print_text_report(report)

    if args.advisory:
        return 0
    return 1 if has_gaps(report) else 0


if __name__ == "__main__":
    raise SystemExit(main())

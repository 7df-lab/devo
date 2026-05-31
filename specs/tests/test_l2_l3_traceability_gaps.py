from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPT = REPO_ROOT / "specs" / "l2_l3_traceability_gaps.py"


def load_script():
    spec = importlib.util.spec_from_file_location("l2_l3_traceability_gaps", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def spec_file(artifact_id: str, revision: int, title: str) -> str:
    return f"""---
artifact_id: {artifact_id}
revision: {revision}
---

# {title}
"""


class L2L3TraceabilityTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.repo = Path(self.tmp.name)
        (self.repo / "specs" / "traceability").mkdir(parents=True)
        (self.repo / "specs" / "L2").mkdir(parents=True)
        (self.repo / "specs" / "L3").mkdir(parents=True)

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def run_script(self, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--repo", str(self.repo), *args],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )

    def add_l2(self, name: str, body: str = "") -> None:
        write(
            self.repo / "specs" / "L2" / f"{name}.md",
            spec_file(name, 1, name) + body,
        )

    def add_l3(self, name: str, revision: int = 1) -> None:
        write(
            self.repo / "specs" / "L3" / f"{name}.md",
            spec_file(name, revision, name),
        )

    def write_matrix(self, rows: list[str]) -> None:
        write(
            self.repo / "specs" / "traceability" / "l2_to_l3.md",
            "\n".join(
                [
                    "# L2 to L3 Traceability Matrix",
                    "",
                    "| Source ID | Source Path | Target ID | Target Path | Relationship | Rationale |",
                    "|---|---|---|---|---|---|",
                    *rows,
                    "",
                ]
            ),
        )

    def test_classifies_unlinked_related_only_and_primary_linked(self) -> None:
        self.add_l2("L2-DES-APP-001")
        self.add_l2("L2-DES-APP-002")
        self.add_l2("L2-DES-APP-003")
        self.add_l3("L3-BEH-APP-001")
        self.add_l3("L3-BEH-APP-002")
        self.write_matrix(
            [
                "| L2-DES-APP-001 | specs/L2/L2-DES-APP-001.md | L3-BEH-APP-001 | specs/L3/L3-BEH-APP-001.md | specified-by | primary |",
                "| L2-DES-APP-002 | specs/L2/L2-DES-APP-002.md | L3-BEH-APP-002 | specs/L3/L3-BEH-APP-002.md | related-to | secondary |",
            ]
        )

        result = self.run_script("--json", "--advisory")

        self.assertEqual(result.returncode, 0, result.stderr)
        payload = json.loads(result.stdout)
        self.assertEqual(payload["counts"]["source_total"], 3)
        self.assertEqual(payload["counts"]["primary_linked"], 1)
        self.assertEqual(payload["counts"]["related_only"], 1)
        self.assertEqual(payload["counts"]["unlinked"], 1)

    def test_stale_target_does_not_count_as_primary_coverage(self) -> None:
        self.add_l2("L2-DES-APP-001")
        self.write_matrix(
            [
                "| L2-DES-APP-001 | specs/L2/L2-DES-APP-001.md | L3-BEH-APP-999 | specs/L3/L3-BEH-APP-999.md | specified-by | stale |",
            ]
        )

        result = self.run_script("--json", "--advisory")

        payload = json.loads(result.stdout)
        self.assertEqual(payload["counts"]["primary_linked"], 0)
        self.assertEqual(payload["counts"]["unlinked"], 1)
        self.assertEqual(payload["counts"]["stale_targets"], 1)

    def test_reports_malformed_rows_with_line_numbers_and_duplicate_rows(self) -> None:
        self.add_l2("L2-DES-APP-001")
        self.add_l3("L3-BEH-APP-001")
        row = "| L2-DES-APP-001 | specs/L2/L2-DES-APP-001.md | L3-BEH-APP-001 | specs/L3/L3-BEH-APP-001.md | specified-by | rationale |"
        self.write_matrix([row, row, "| L2-DES-APP-001 | too few | cells |"])

        result = self.run_script("--json", "--advisory")

        payload = json.loads(result.stdout)
        self.assertEqual(payload["counts"]["duplicate_rows"], 1)
        self.assertEqual(payload["counts"]["malformed_rows"], 1)
        self.assertEqual(payload["malformed_rows"][0]["line_number"], 7)

    def test_embedded_tbd_and_revision_drift_are_reported(self) -> None:
        self.add_l2(
            "L2-DES-APP-001",
            """
## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---:|---|---|---|
| specified-by | TBD | TBD | TBD | pending |
| specified-by | L3-BEH-APP-001 | 1 | specs/L3/L3-BEH-APP-001.md | stale revision |
""",
        )
        self.add_l3("L3-BEH-APP-001", revision=2)
        self.write_matrix(
            [
                "| L2-DES-APP-001 | specs/L2/L2-DES-APP-001.md | L3-BEH-APP-001 | specs/L3/L3-BEH-APP-001.md | specified-by | primary |",
            ]
        )

        result = self.run_script("--json", "--advisory")

        payload = json.loads(result.stdout)
        drift_kinds = {drift["kind"] for drift in payload["embedded_trace_drifts"]}
        self.assertEqual(drift_kinds, {"embedded_trace_stale_tbd", "embedded_trace_revision_drift"})

    def test_embedded_missing_matrix_target_is_reported_for_each_target(self) -> None:
        self.add_l2(
            "L2-DES-APP-001",
            """
## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---:|---|---|---|
| specified-by | L3-BEH-APP-001 | 1 | specs/L3/L3-BEH-APP-001.md | present |
""",
        )
        self.add_l3("L3-BEH-APP-001")
        self.add_l3("L3-BEH-APP-002")
        self.write_matrix(
            [
                "| L2-DES-APP-001 | specs/L2/L2-DES-APP-001.md | L3-BEH-APP-001 | specs/L3/L3-BEH-APP-001.md | specified-by | primary |",
                "| L2-DES-APP-001 | specs/L2/L2-DES-APP-001.md | L3-BEH-APP-002 | specs/L3/L3-BEH-APP-002.md | specified-by | second |",
            ]
        )

        result = self.run_script("--json", "--advisory")

        payload = json.loads(result.stdout)
        self.assertEqual(payload["counts"]["embedded_trace_drifts"], 1)
        self.assertEqual(payload["embedded_trace_drifts"][0]["kind"], "embedded_trace_missing")
        self.assertEqual(payload["embedded_trace_drifts"][0]["target_id"], "L3-BEH-APP-002")

    def test_embedded_extra_target_not_in_matrix_is_reported(self) -> None:
        self.add_l2(
            "L2-DES-APP-001",
            """
## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---:|---|---|---|
| specified-by | L3-BEH-APP-001 | 1 | specs/L3/L3-BEH-APP-001.md | present |
| specified-by | L3-BEH-APP-002 | 1 | specs/L3/L3-BEH-APP-002.md | extra |
""",
        )
        self.add_l3("L3-BEH-APP-001")
        self.add_l3("L3-BEH-APP-002")
        self.write_matrix(
            [
                "| L2-DES-APP-001 | specs/L2/L2-DES-APP-001.md | L3-BEH-APP-001 | specs/L3/L3-BEH-APP-001.md | specified-by | primary |",
            ]
        )

        result = self.run_script("--json", "--advisory")

        payload = json.loads(result.stdout)
        self.assertEqual(payload["counts"]["embedded_trace_drifts"], 1)
        self.assertEqual(payload["embedded_trace_drifts"][0]["kind"], "embedded_trace_extra")
        self.assertEqual(payload["embedded_trace_drifts"][0]["target_id"], "L3-BEH-APP-002")

    def test_blocking_mode_returns_one_for_gaps_and_advisory_returns_zero(self) -> None:
        self.add_l2("L2-DES-APP-001")
        self.write_matrix([])

        blocking = self.run_script("--json")
        advisory = self.run_script("--json", "--advisory")

        self.assertEqual(blocking.returncode, 1)
        self.assertEqual(advisory.returncode, 0)

    def test_duplicate_artifact_ids_are_usage_errors(self) -> None:
        write(
            self.repo / "specs" / "L2" / "one.md",
            spec_file("L2-DES-APP-001", 1, "one"),
        )
        write(
            self.repo / "specs" / "L2" / "two.md",
            spec_file("L2-DES-APP-001", 1, "two"),
        )
        self.write_matrix([])

        result = self.run_script("--json", "--advisory")

        self.assertEqual(result.returncode, 2)
        self.assertIn("Duplicate", result.stderr)

    def test_markdown_cells_preserves_escaped_pipes(self) -> None:
        module = load_script()

        cells = module.markdown_cells(r"| a | b \| c | d |", line_number=9)

        self.assertEqual([cell.value for cell in cells], ["a", "b | c", "d"])


if __name__ == "__main__":
    unittest.main()

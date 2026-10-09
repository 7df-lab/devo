import io
import tempfile
import unittest
from pathlib import Path

import dill
from rlm.repl import _restore_state, _snapshot_state


class SnapshotFileTests(unittest.TestCase):
    def test_snapshot_skips_open_closed_and_nested_file_handles(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            edited = root / "example.txt"
            with edited.open("w", encoding="utf-8") as closed_handle:
                closed_handle.write("Desktop QA edit succeeded.")
            with edited.open("a", encoding="utf-8") as open_handle:
                namespace = {
                    "closed_handle": closed_handle,
                    "nested": {"file": closed_handle},
                    "open_handle": open_handle,
                    "answer": 42,
                    "buffer": io.BytesIO(b"in-memory state"),
                }
                snapshot = root / "kernel.dill"
                result = _snapshot_state(
                    namespace, str(snapshot), str(root / "manifest.json"),
                    1024 * 1024, 1024 * 1024, False,
                )
            self.assertNotIn("error", result)
            self.assertEqual(
                {entry["name"] for entry in result["skipped"]},
                {"closed_handle", "nested", "open_handle"},
            )
            restored = {}
            self.assertEqual(
                _restore_state(restored, str(snapshot)),
                {"restored": ["answer", "buffer"], "failed": []},
            )
            self.assertEqual(
                {"answer": restored["answer"], "buffer": restored["buffer"].getvalue(),
                 "file": edited.read_text(encoding="utf-8")},
                {"answer": 42, "buffer": b"in-memory state", "file": "Desktop QA edit succeeded."},
            )

    def test_legacy_closed_write_handle_is_rejected_before_reopening(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            edited = root / "example.txt"
            with edited.open("w", encoding="utf-8") as handle:
                handle.write("Preserve this file after restart.")
            # The old snapshot format contains ordinary dill blobs, including
            # file reducers. Keep other bindings when one legacy binding fails.
            payload = {"answer": dill.dumps(42), "file": dill.dumps({"nested": handle})}
            snapshot = root / "kernel.dill"
            snapshot.write_bytes(dill.dumps(payload))
            restored = {}
            self.assertEqual(
                _restore_state(restored, str(snapshot)),
                {"restored": ["answer"], "failed": [{
                    "name": "file",
                    "reason": "UnpicklingError: OS file handles cannot be restored safely",
                }]},
            )
            self.assertEqual(
                {"namespace": restored, "file": edited.read_text(encoding="utf-8")},
                {"namespace": {"answer": 42}, "file": "Preserve this file after restart."},
            )

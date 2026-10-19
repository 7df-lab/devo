import os
import tempfile
import unittest
import uuid
from pathlib import Path
from unittest.mock import patch

from rlm._snapshot_files import create_snapshot_temp
from rlm.repl import _snapshot_state


class SnapshotStagingTests(unittest.TestCase):
    def test_staging_is_exclusive_and_in_the_target_directory(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "kernel.dill"
            first_fd, first_path = create_snapshot_temp(str(target))
            second_fd, second_path = create_snapshot_temp(str(target))
            with os.fdopen(first_fd, "wb") as first, os.fdopen(second_fd, "wb") as second:
                first.write(b"first")
                second.write(b"second")
            self.assertNotEqual(first_path, second_path)
            self.assertEqual(
                [Path(first_path).parent, Path(second_path).parent], [target.parent, target.parent]
            )
            self.assertEqual(
                [Path(first_path).read_bytes(), Path(second_path).read_bytes()], [b"first", b"second"]
            )
            self.assertFalse(target.exists())

    def test_permission_denial_is_not_retried(self):
        target = str(Path.cwd() / "kernel.dill")
        denied = PermissionError("sandbox denied staging")
        with patch("rlm._snapshot_files.os.open", side_effect=denied) as opening:
            with self.assertRaises(PermissionError) as raised:
                create_snapshot_temp(target)
        self.assertIs(raised.exception, denied)
        self.assertEqual(opening.call_count, 1)

    def test_real_collision_preserves_existing_file_and_retries(self):
        with tempfile.TemporaryDirectory() as directory:
            target = str(Path(directory) / "kernel.dill")
            existing = Path(f"{target}.{uuid.UUID(int=1).hex}.tmp")
            existing.write_bytes(b"preserve")
            with patch("rlm._snapshot_files.uuid.uuid4", side_effect=[uuid.UUID(int=1), uuid.UUID(int=2)]):
                fd, path = create_snapshot_temp(target)
            os.close(fd)
            self.assertEqual(Path(path), Path(f"{target}.{uuid.UUID(int=2).hex}.tmp"))
            self.assertEqual(existing.read_bytes(), b"preserve")

    def test_repeated_collisions_are_bounded(self):
        target = str(Path.cwd() / "kernel.dill")
        with patch("rlm._snapshot_files.os.open", side_effect=FileExistsError) as opening:
            with self.assertRaises(FileExistsError):
                create_snapshot_temp(target)
        self.assertEqual(opening.call_count, 8)

    def test_denied_manifest_staging_preserves_previous_snapshot(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            snapshot = root / "kernel.dill"
            manifest = root / "manifest.json"
            snapshot.write_bytes(b"previous snapshot")
            manifest.write_bytes(b"previous manifest")

            def stage(target):
                if target == str(manifest):
                    raise PermissionError("sandbox denied staging")
                return create_snapshot_temp(target)

            with patch("rlm.repl.create_snapshot_temp", side_effect=stage):
                result = _snapshot_state(
                    {"answer": 42}, str(snapshot), str(manifest),
                    1024 * 1024, 1024 * 1024, False,
                )
            self.assertEqual(result, {"error": "manifest write failed: sandbox denied staging"})
            self.assertEqual(
                [snapshot.read_bytes(), manifest.read_bytes(), sorted(p.name for p in root.iterdir())],
                [b"previous snapshot", b"previous manifest", ["kernel.dill", "manifest.json"]],
            )


if __name__ == "__main__":
    unittest.main()

import os
import subprocess
import tempfile
import unittest
from pathlib import Path

from rlm.windows_shell import find_windows_bash


@unittest.skipUnless(os.name == "nt", "Git Bash temp mounts are Windows-specific")
class WindowsShellTempTests(unittest.TestCase):
    def test_native_temp_environment_maps_shell_tmp_to_owned_directory(self):
        shell = find_windows_bash(os.environ)
        if shell is None:
            self.skipTest("Git for Windows is not installed")
        with tempfile.TemporaryDirectory() as directory:
            owned = Path(directory) / "session scratch"
            owned.mkdir()
            environment = dict(os.environ)
            environment.update({key: str(owned) for key in ("TEMP", "TMP", "TMPDIR")})
            result = subprocess.run(
                [shell, "-c", 'f=$(mktemp) || exit; cygpath -w "$f"; '
                 'printf temp-ok > "$f"; cat "$f"; rm "$f"'],
                cwd=owned, env=environment, capture_output=True, encoding="utf-8", timeout=10,
            )
            self.assertEqual((result.returncode, result.stderr), (0, ""))
            native_file, content = result.stdout.splitlines()
            self.assertEqual(
                (Path(native_file).parent, content, list(owned.iterdir())),
                (owned, "temp-ok", []),
            )


if __name__ == "__main__":
    unittest.main()

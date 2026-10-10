import unittest
from importlib import import_module
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch

from rlm.windows_shell import find_windows_bash

bash_module = import_module("rlm.bash")


class WindowsShellTests(unittest.TestCase):
    def test_registered_custom_install_takes_priority(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            custom = root / "Custom Git" / "bin" / "bash.exe"
            custom.parent.mkdir(parents=True)
            custom.touch()
            standard = root / "Programs" / "Git" / "bin" / "bash.exe"
            standard.parent.mkdir(parents=True)
            standard.touch()
            with patch("rlm.windows_shell.git_install_roots", return_value=[str(custom.parent.parent)]):
                self.assertEqual(
                    find_windows_bash({"ProgramFiles": str(root / "Programs")}), str(custom)
                )

    def test_standard_and_per_user_installs(self):
        for key, suffix in (
            ("ProgramFiles", "Git/bin/bash.exe"),
            ("ProgramFiles(x86)", "Git/usr/bin/bash.exe"),
            ("LOCALAPPDATA", "Programs/Git/bin/bash.exe"),
        ):
            with self.subTest(key=key), TemporaryDirectory() as directory:
                candidate = Path(directory) / suffix
                candidate.parent.mkdir(parents=True)
                candidate.touch()
                with patch("rlm.windows_shell.git_install_roots", return_value=[]):
                    self.assertEqual(find_windows_bash({key: directory}), str(candidate))

    def test_does_not_search_path_or_relative_installations(self):
        with TemporaryDirectory() as directory:
            candidate = Path(directory) / "bash.exe"
            candidate.touch()
            with patch("rlm.windows_shell.git_install_roots", return_value=["relative-git"]):
                self.assertIsNone(find_windows_bash({"PATH": directory, "ProgramFiles": "relative"}))

    def test_windows_shell_uses_discovery_without_an_override(self):
        discovered = str(Path.cwd() / "Git" / "bin" / "bash.exe")
        with patch.object(bash_module, "_IS_POSIX", False), patch.object(
            bash_module, "find_windows_bash", return_value=discovered
        ):
            self.assertEqual(bash_module._shell({}), discovered)

    def test_explicit_shell_override_is_preserved(self):
        override = str(Path.cwd() / "my-shell.exe")
        with patch.object(bash_module, "find_windows_bash") as discovery:
            self.assertEqual(bash_module._shell({"DEVO_BASH_SHELL": override}), override)
            discovery.assert_not_called()

    def test_missing_shell_has_actionable_error(self):
        with patch.object(bash_module, "_IS_POSIX", False), patch.object(
            bash_module, "find_windows_bash", return_value=None
        ):
            with self.assertRaisesRegex(RuntimeError, "Install Git for Windows"):
                bash_module._shell({})


if __name__ == "__main__":
    unittest.main()

import subprocess
import sys
import tempfile
from pathlib import Path
import unittest

from ci_scope import COMPONENTS, classify


class CiScopeTests(unittest.TestCase):
    def test_tui_change_does_not_select_rust_or_desktop(self):
        self.assertEqual(classify(["apps/tui/src/host.ts"]), {
            "rust": False, "desktop": False, "tui": True, "kernel_python": False,
        })

    def test_desktop_change_selects_only_desktop(self):
        self.assertEqual(classify(["apps/desktop/package.json"]), {
            "rust": False, "desktop": True, "tui": False, "kernel_python": False,
        })

    def test_rust_contracts_and_embedded_assets_select_all(self):
        for path in ["crates/protocol/src/lib.rs", "crates/skills/assets/bundled/goal/SKILL.md",
                     "crates/kernel/rlm-runtime/src/rlm/repl.py"]:
            with self.subTest(path=path):
                self.assertEqual(classify([path]), dict.fromkeys(COMPONENTS, True))

    def test_unknown_and_build_configuration_paths_select_all(self):
        for path in ["Cargo.toml", "Cargo.lock", ".cargo/config.toml", "rust-toolchain.toml",
                     ".github/workflows/ci.yml", "scripts/ci_scope.py", "new-component/file.txt"]:
            with self.subTest(path=path):
                self.assertEqual(classify([path]), dict.fromkeys(COMPONENTS, True))

    def test_documentation_and_web_changes_need_no_rust(self):
        self.assertEqual(classify(["README.md", "README.ja.md", "CONTRIBUTING.md", "AGENTS.md",
                                   "docs/build.md", "apps/web/package.json"]),
                         dict.fromkeys(COMPONENTS, False))

    def test_mixed_changes_union_components(self):
        self.assertEqual(classify(["apps/tui/src/host.ts", "apps/desktop/src/main.ts"]), {
            "rust": False, "desktop": True, "tui": True, "kernel_python": False,
        })

    def test_rename_out_of_rust_keeps_old_path(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary)
            for command in [["git", "init", "--quiet"], ["git", "config", "user.email", "qa@example.test"],
                            ["git", "config", "user.name", "QA"]]:
                subprocess.run(command, cwd=repo, check=True, capture_output=True)
            source = repo / "crates" / "example" / "lib.rs"
            source.parent.mkdir(parents=True)
            source.write_text("// fixture\n")
            subprocess.run(["git", "add", "."], cwd=repo, check=True, capture_output=True)
            subprocess.run(["git", "commit", "--quiet", "-m", "fixture"], cwd=repo, check=True, capture_output=True)
            destination = repo / "apps" / "tui" / "fixture.ts"
            destination.parent.mkdir(parents=True)
            source.rename(destination)
            subprocess.run(["git", "add", "-A"], cwd=repo, check=True, capture_output=True)
            subprocess.run(["git", "commit", "--quiet", "-m", "move"], cwd=repo, check=True, capture_output=True)
            result = subprocess.run(
                [sys.executable, str(Path(__file__).with_name("ci_scope.py")), "--base", "HEAD~1"],
                cwd=repo, check=True, capture_output=True, text=True,
            )
            self.assertEqual(result.stdout, "rust=true\ndesktop=true\ntui=true\nkernel_python=true\n")

    def test_missing_diff_selects_full_ci_and_appends_outputs(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "outputs"
            output.write_text("existing=value\n", encoding="utf-8")
            subprocess.run(
                [sys.executable, str(Path(__file__).with_name("ci_scope.py")),
                 "--base", "missing-ref-for-ci-scope-test", "--output", str(output)],
                check=True, capture_output=True,
            )
            self.assertEqual(output.read_text(encoding="utf-8"),
                             "existing=value\nrust=true\ndesktop=true\ntui=true\nkernel_python=true\n")

    def test_main_branch_mode_always_selects_full_ci(self):
        result = subprocess.run(
            [sys.executable, str(Path(__file__).with_name("ci_scope.py")), "--all"],
            check=True, capture_output=True, text=True,
        )
        self.assertEqual(result.stdout, "rust=true\ndesktop=true\ntui=true\nkernel_python=true\n")


if __name__ == "__main__":
    unittest.main()

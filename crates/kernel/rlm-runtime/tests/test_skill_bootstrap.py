import asyncio
import sys
import tempfile
import unittest
from pathlib import Path

from rlm._skill_bootstrap import sync_skills


class SkillBootstrapTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.namespace = {}
        self.addCleanup(self.temp.cleanup)
        self.addCleanup(lambda: sync_skills(self.namespace, []))

    def package(self, name="selected_skill", body="def run(): return 42", root=None):
        directory = (root or self.root) / name
        directory.mkdir(parents=True)
        (directory / "__init__.py").write_text(body, encoding="utf-8")
        return {"module": name, "package_dir": str(directory), "skill_path": str(directory / "SKILL.md")}

    def test_exact_selected_package_and_callable_wrapper(self):
        selected = self.package()
        ambient = self.package(body="raise AssertionError('ambient package executed')", root=self.root / "ambient")
        sys.path.insert(0, str(Path(ambient["package_dir"]).parent))
        self.addCleanup(sys.path.remove, sys.path[0])
        self.assertEqual(sync_skills(self.namespace, [selected]), ["selected_skill"])
        self.assertEqual(asyncio.run(self.namespace["selected_skill"]()), 42)
        self.assertEqual(self.namespace["selected_skill"].__file__, str(Path(selected["package_dir"]) / "__init__.py"))

    def test_disable_reenable_without_resetting_user_namespace(self):
        selected = self.package(body="value = []\nasync def run(): return len(value)")
        self.namespace["user_variable"] = 123
        sync_skills(self.namespace, [selected])
        module = self.namespace["selected_skill"]
        module.value.append("keep live state")
        sync_skills(self.namespace, [selected])
        self.assertIs(self.namespace["selected_skill"], module)
        self.assertEqual(asyncio.run(module()), 1)
        self.assertEqual(sync_skills(self.namespace, []), [])
        self.assertNotIn("selected_skill", self.namespace)
        self.assertNotIn("selected_skill", sys.modules)
        self.assertEqual(self.namespace["user_variable"], 123)
        self.assertEqual(sync_skills(self.namespace, [selected]), ["selected_skill"])

    def test_disable_preserves_user_rebinding_and_existing_reference(self):
        selected = self.package()
        sync_skills(self.namespace, [selected])
        reference = self.namespace["selected_skill"]
        self.namespace["selected_skill"] = "user-owned"
        self.assertEqual(sync_skills(self.namespace, [selected]), [])
        sync_skills(self.namespace, [])
        self.assertEqual(self.namespace["selected_skill"], "user-owned")
        self.assertEqual(asyncio.run(reference()), 42)

    def test_failed_import_not_reported_or_resolved_from_ambient_module(self):
        selected = self.package(body="import definitely_missing_devo_test_dependency")
        self.assertEqual(sync_skills(self.namespace, [selected]), [])
        self.assertNotIn("selected_skill", self.namespace)
        self.assertNotIn("selected_skill", sys.modules)
        self.assertIn("definitely_missing", self.namespace["_devo_selected_skill_import_error"])

    def test_identity_change_replaces_only_owned_binding(self):
        first = self.package()
        second = self.package(body="def run(): return 99", root=self.root / "different")
        sync_skills(self.namespace, [first])
        sync_skills(self.namespace, [second])
        self.assertEqual(asyncio.run(self.namespace["selected_skill"]()), 99)

    def test_relative_imports_work_and_are_removed_on_disable(self):
        selected = self.package(body="from .helper import value\ndef run(): return value")
        (Path(selected["package_dir"]) / "helper.py").write_text("value = 7", encoding="utf-8")
        sync_skills(self.namespace, [selected])
        self.assertEqual(asyncio.run(self.namespace["selected_skill"]()), 7)
        sync_skills(self.namespace, [])
        self.assertNotIn("selected_skill.helper", sys.modules)

    def test_empty_manifest_does_not_discover_or_execute_nearby_packages(self):
        self.package(body="raise AssertionError('unselected executed')")
        self.assertEqual(sync_skills(self.namespace, []), [])
        self.assertNotIn("selected_skill", self.namespace)

    def test_selected_modules_are_registered_before_snapshot_alias_restore(self):
        from rlm.repl import _snapshot_state, _restore_state

        selected = self.package(body="def read(): return 42")
        sync_skills(self.namespace, [selected])
        self.namespace["saved_module"] = self.namespace["selected_skill"]
        self.namespace["saved_function"] = self.namespace["selected_skill"].read
        self.namespace["user_answer"] = 42
        snapshot, manifest = self.root / "kernel.dill", self.root / "kernel.json"
        result = _snapshot_state(self.namespace, str(snapshot), str(manifest), 1_000_000, 1_000_000, False)
        self.assertNotIn("error", result)
        sync_skills(self.namespace, [])
        restored = {}
        self.addCleanup(lambda: sync_skills(restored, []))
        sync_skills(restored, [selected])  # host registers exact packages before dill
        result = _restore_state(restored, str(snapshot))
        self.assertEqual(result["failed"], [])
        sync_skills(restored, [selected])
        self.assertEqual((restored["saved_module"].read(), restored["saved_function"](), restored["user_answer"]), (42, 42, 42))


if __name__ == "__main__":
    unittest.main()

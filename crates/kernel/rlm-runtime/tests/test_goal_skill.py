"""The bundled goal API advertises and routes the full requested lifecycle."""
import asyncio
import importlib.util
from pathlib import Path
import sys
import types
import unittest
from unittest.mock import AsyncMock, patch


ROOT = Path(__file__).resolve().parents[3]
SKILL = ROOT / "skills/assets/bundled/goal"


class GoalSkillTests(unittest.TestCase):
    def test_goal_controls_use_host_bridge(self):
        bridge = AsyncMock(return_value={"status": "ok"})
        rlm = types.ModuleType("rlm")
        rlm.host_request = bridge
        spec = importlib.util.spec_from_file_location("goal_under_test", SKILL / "src/goal/__init__.py")
        goal = importlib.util.module_from_spec(spec)
        with patch.dict(sys.modules, {"rlm": rlm}):
            spec.loader.exec_module(goal)
        for action in ("get", "pause", "resume", "cancel", "complete"):
            with self.subTest(action=action):
                self.assertEqual(asyncio.run(getattr(goal, action)()), {"status": "ok"})
                bridge.assert_awaited_with(f"goal.{action}")
        asyncio.run(goal.create("Ship release"))
        bridge.assert_awaited_with("goal.create", {"objective": "Ship release"})
        description = (SKILL / "SKILL.md").read_text()
        for action in ("pause", "resume", "cancel"):
            self.assertIn(f"await goal.{action}()", description)
        self.assertNotIn("there is no API for", description)


if __name__ == "__main__":
    unittest.main()

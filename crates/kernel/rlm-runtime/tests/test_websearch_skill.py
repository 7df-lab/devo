"""The bundled websearch skill only uses the host's selected route."""
import importlib.util
from pathlib import Path
import sys
import types
import unittest
from unittest.mock import AsyncMock, patch


ROOT = Path(__file__).resolve().parents[3]
SKILL = ROOT / "skills/assets/bundled/websearch/src/websearch/websearch.py"
SPEC = importlib.util.spec_from_file_location("websearch_under_test", SKILL)
websearch = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(websearch)


class WebsearchSkillTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.search = AsyncMock()
        rlm = types.ModuleType("rlm")
        rlm.web_search = self.search
        modules = patch.dict(sys.modules, {"rlm": rlm})
        modules.start()
        self.addCleanup(modules.stop)

    async def test_formats_local_and_mcp_results_from_the_selected_route(self):
        self.search.side_effect = [
            {"stdout": "legacy local result"},
            {"content": [{"type": "text", "text": "one"}, {"type": "text", "text": "two"}]},
            {"content": [], "structuredContent": {"results": ["structured"]}},
        ]
        self.assertEqual(
            await websearch.run("docs", num_results=5, timeout=1), "legacy local result"
        )
        self.search.assert_awaited_once_with("docs")
        self.assertEqual(await websearch.run("docs"), "one\n\ntwo")
        self.assertIn('"results": ["structured"]', await websearch.run("docs"))

    async def test_off_unconfigured_denied_and_backend_errors_do_not_fall_back(self):
        for error in (
            RuntimeError("MCP or local web search is not configured for this turn. Check /web."),
            RuntimeError("API key required"),
            PermissionError("denied"),
            RuntimeError("selected MCP search failed"),
        ):
            with self.subTest(error=error):
                self.search.reset_mock()
                self.search.side_effect = error
                with self.assertRaises(type(error)) as raised:
                    await websearch.run("docs")
                self.assertIs(raised.exception, error)
                self.search.assert_awaited_once_with("docs")

    async def test_empty_results_and_small_output_limit_do_not_retry(self):
        self.search.return_value = {"content": []}
        self.assertEqual(
            await websearch.run("docs"), "No text results returned by the selected web search backend."
        )
        self.search.assert_awaited_once_with("docs")
        self.search.return_value = {"stdout": "a" * 100}
        self.assertEqual(await websearch.run("docs", max_output=3), "aaa")
        self.assertEqual(await websearch.run("docs", max_output=0), "")


if __name__ == "__main__":
    unittest.main()

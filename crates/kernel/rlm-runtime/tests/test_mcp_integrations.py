"""Authored MCP integrations remain separate from the generic host facade."""

import importlib.util
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import AsyncMock, patch

import rlm
from rlm import mcp_base
import rlm.repl as repl


class AuthoredMcpIntegrationTests(unittest.IsolatedAsyncioTestCase):
    async def test_bundled_integrations_keep_discovery_and_parsed_results(self):
        bundled = Path(__file__).resolve().parents[3] / "skills" / "assets" / "bundled"
        for name, endpoint, tool in (
            ("linear", "https://mcp.linear.app/mcp", "list_issues"),
            ("notion", "https://mcp.notion.com/mcp", "search"),
        ):
            with self.subTest(integration=name):
                spec = importlib.util.spec_from_file_location(
                    f"test_bundled_{name}", bundled / name / "src" / name / "__init__.py"
                )
                module = importlib.util.module_from_spec(spec)
                spec.loader.exec_module(module)
                integration = getattr(module, name)
                self.assertIsInstance(integration, rlm.McpIntegration)
                self.assertEqual((integration.server, integration.url), (name, endpoint))
                schema = {"type": "object", "properties": {"query": {"type": "string"}}}
                session = SimpleNamespace(
                    list_tools=AsyncMock(return_value=SimpleNamespace(tools=[
                        SimpleNamespace(name=tool, description="Search", input_schema=schema)
                    ])),
                    call_tool=AsyncMock(return_value=SimpleNamespace(
                        content=[], structured_content={"items": ["found"]}, is_error=False
                    )),
                )
                host = AsyncMock(side_effect=AssertionError("authored integrations changed route"))
                with (
                    patch.object(integration, "_open_session", AsyncMock(return_value=session)),
                    patch.object(repl, "host_request", host),
                ):
                    result = await getattr(module, tool)(query="roadmap")
                    tools = await integration.list_tools()
                self.assertEqual(result, {"items": ["found"]})
                self.assertEqual(tools, [{"name": tool, "description": "Search", "inputSchema": schema}])
                self.assertEqual(session.list_tools.await_count, 2)
                session.call_tool.assert_awaited_once_with(tool, {"query": "roadmap"})
                host.assert_not_awaited()
                for reserved in ("run", "__wrapped__", "__call__"):
                    self.assertFalse(hasattr(module, reserved))

    def test_authored_integration_tool_errors_still_raise(self):
        result = SimpleNamespace(content=[SimpleNamespace(text="tool failed")], is_error=True)
        with self.assertRaisesRegex(mcp_base.McpToolError, "tool failed"):
            mcp_base._parse_result(result)


if __name__ == "__main__":
    unittest.main()

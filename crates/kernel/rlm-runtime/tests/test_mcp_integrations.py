"""Authored MCP integrations remain separate from the generic host facade."""

from types import SimpleNamespace
import unittest
from unittest.mock import AsyncMock, patch

from rlm import McpIntegration, mcp_base, repl


class SampleIntegration(McpIntegration):
    server = "sample"
    url = "https://sample.invalid/mcp"


class AuthoredMcpIntegrationTests(unittest.IsolatedAsyncioTestCase):
    async def test_authored_integration_discovers_tools_and_parses_results(self):
        integration = SampleIntegration()
        schema = {"type": "object", "properties": {"query": {"type": "string"}}}
        session = SimpleNamespace(
            list_tools=AsyncMock(return_value=SimpleNamespace(tools=[
                SimpleNamespace(name="search", description="Search", input_schema=schema)
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
            result = await integration.search(query="roadmap")
            tools = await integration.list_tools()
        self.assertEqual(result, {"items": ["found"]})
        self.assertEqual(tools, [{"name": "search", "description": "Search", "inputSchema": schema}])
        self.assertEqual(session.list_tools.await_count, 2)
        session.call_tool.assert_awaited_once_with("search", {"query": "roadmap"})
        host.assert_not_awaited()

    def test_authored_integration_tool_errors_still_raise(self):
        result = SimpleNamespace(content=[SimpleNamespace(text="tool failed")], is_error=True)
        with self.assertRaisesRegex(mcp_base.McpToolError, "tool failed"):
            mcp_base._parse_result(result)


if __name__ == "__main__":
    unittest.main()

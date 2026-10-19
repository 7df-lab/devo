import unittest
from unittest.mock import AsyncMock, patch

import rlm.mcp as mcp
import rlm.repl as repl


class McpHostListToolsTests(unittest.IsolatedAsyncioTestCase):
    async def test_native_list_tools_uses_exact_server_and_python_contract(self):
        calls = []
        expected_schema = {"type": "object", "properties": {"q": {"type": "string"}}}

        async def fake_host_request(data):
            calls.append(data)
            return {
                "status": "ok",
                "result": {
                    "tools": [
                        {
                            "server": "docs",
                            "name": "search:raw/name",
                            "description": "Search docs",
                            "inputSchema": expected_schema,
                            "flatName": "must_not_escape",
                        },
                        {
                            "server": "Docs",
                            "name": "wrong-case",
                            "description": "Other server",
                            "inputSchema": {},
                        },
                        {
                            "server": "docs-extra",
                            "name": "wrong-prefix",
                            "description": "Not an exact match",
                            "inputSchema": {},
                        },
                    ]
                },
            }

        with (
            patch.object(repl, "host_request", new=fake_host_request),
            patch.object(mcp, "_dispatch", new=AsyncMock(side_effect=AssertionError("fallback used"))),
        ):
            result = await mcp.list_tools("docs")

        self.assertEqual(calls, [{"server": "docs", "type": "mcp.list_tools"}])
        self.assertEqual(
            result,
            [
                {
                    "name": "search:raw/name",
                    "description": "Search docs",
                    "inputSchema": expected_schema,
                }
            ],
        )
        self.assertEqual(set(result[0]), {"name", "description", "inputSchema"})

    async def test_server_and_permission_errors_do_not_fall_back(self):
        fallback = AsyncMock(side_effect=AssertionError("fallback used"))
        for message in ("mcp server unavailable: docs", "Permission denied for docs"):
            async def fake_host_request(_data, error=message):
                return {"status": "error", "error": error}

            with (
                patch.object(repl, "host_request", new=fake_host_request),
                patch.object(mcp, "_dispatch", new=fallback),
            ):
                with self.assertRaisesRegex(RuntimeError, message):
                    await mcp.list_tools("docs")
        fallback.assert_not_awaited()

    async def test_unsupported_host_action_uses_local_fallback(self):
        async def fake_host_request(_data):
            return {
                "status": "error",
                "error": "unknown host_request action: mcp.list_tools",
            }

        local_tools = [{"name": "local", "description": "Local", "inputSchema": {}}]
        fallback = AsyncMock(return_value=local_tools)
        with (
            patch.object(repl, "host_request", new=fake_host_request),
            patch.object(mcp, "_dispatch", new=fallback),
        ):
            result = await mcp.list_tools("docs")
        self.assertIs(result, local_tools)
        fallback.assert_awaited_once()

    async def test_unavailable_host_uses_local_fallback(self):
        async def fake_host_request(_data):
            raise RuntimeError("host connection closed")

        local_tools = [{"name": "local", "description": "Local", "inputSchema": {}}]
        fallback = AsyncMock(return_value=local_tools)
        with (
            patch.object(repl, "host_request", new=fake_host_request),
            patch.object(mcp, "_dispatch", new=fallback),
        ):
            result = await mcp.list_tools("docs")
        self.assertIs(result, local_tools)
        fallback.assert_awaited_once()


if __name__ == "__main__":
    unittest.main()

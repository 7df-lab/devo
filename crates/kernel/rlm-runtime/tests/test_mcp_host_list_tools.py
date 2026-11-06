import asyncio
import unittest
from unittest.mock import AsyncMock, patch

import rlm.mcp as mcp
import rlm.repl as repl


class McpHostTests(unittest.IsolatedAsyncioTestCase):
    async def test_native_list_tools_uses_exact_server_and_python_contract(self):
        schema = {"type": "object", "properties": {"q": {"type": "string"}}}
        host = AsyncMock(return_value={"status": "ok", "result": {"tools": [
            {"server": "docs", "name": "search:raw/name", "description": "Search docs",
             "inputSchema": schema, "flatName": "must_not_escape"},
            {"server": "Docs", "name": "wrong-case"},
            {"server": "docs-extra", "name": "wrong-prefix"},
            {"server": "docs", "name": None},
            {"server": "docs", "name": "defaults", "description": None, "inputSchema": []},
            None,
        ]}})
        with patch.object(repl, "host_request", host):
            result = await mcp.list_tools("docs")
        host.assert_awaited_once_with({"server": "docs", "type": "mcp.list_tools"})
        self.assertEqual(result, [
            {"name": "search:raw/name", "description": "Search docs", "inputSchema": schema},
            {"name": "defaults", "description": "", "inputSchema": {}},
        ])

    async def test_call_preserves_complete_mcp_results(self):
        results = [
            {"content": [{"type": "text", "text": "plain text"}], "isError": False},
            {"content": [{"type": "text", "text": '{"answer":42}'}],
             "structuredContent": {"answer": 42}, "isError": False, "_meta": {"trace": "id"}},
            {"content": [{"type": "image", "data": "aGVsbG8=", "mimeType": "image/png"}]},
            {"content": [{"type": "text", "text": "tool failed"}], "isError": True},
            {"content": []},
        ]
        for expected in results:
            with self.subTest(expected=expected):
                host = AsyncMock(return_value={"status": "ok", "result": expected})
                with patch.object(repl, "host_request", host):
                    actual = await mcp.call_tool("docs", "search:raw/name", {"q": "test"})
                self.assertIs(actual, expected)
                host.assert_awaited_once_with({"type": "mcp.call", "server": "docs",
                                              "tool": "search:raw/name", "arguments": {"q": "test"}})

    async def test_call_defaults_arguments_to_empty_object(self):
        host = AsyncMock(return_value={"status": "ok", "result": {"content": []}})
        with patch.object(repl, "host_request", host):
            self.assertEqual(await mcp.call_tool("docs", "list"), {"content": []})
        host.assert_awaited_once_with({"type": "mcp.call", "server": "docs",
                                      "tool": "list", "arguments": {}})

    async def test_host_rejections_remain_visible_without_retry(self):
        for message in ("mcp server unavailable: docs", "Permission denied for docs",
                        "MCP server 'docs' is disabled", "plan mode denies mcp.call"):
            for call, error_type in ((lambda: mcp.list_tools("docs"), RuntimeError),
                                     (lambda: mcp.call_tool("docs", "search"), PermissionError)):
                with self.subTest(message=message, error_type=error_type):
                    host = AsyncMock(return_value={"status": "error", "error": message})
                    with patch.object(repl, "host_request", host):
                        with self.assertRaises(error_type) as error:
                            await call()
                    self.assertEqual(str(error.exception), message)
                    host.assert_awaited_once()

    async def test_unsupported_host_requires_upgrade(self):
        for action, call in (("mcp.list_tools", lambda: mcp.list_tools("docs")),
                             ("mcp.call", lambda: mcp.call_tool("docs", "search"))):
            for kind in ("unknown", "unsupported"):
                for label in ("host_request action", "host request action", "action"):
                    host = AsyncMock(return_value={"status": "error",
                                                   "error": f"{kind} {label}: {action}"})
                    with self.subTest(action=action, kind=kind, label=label):
                        with patch.object(repl, "host_request", host):
                            with self.assertRaisesRegex(RuntimeError, "upgrade Devo and restart"):
                                await call()
                        host.assert_awaited_once()

    async def test_transport_errors_and_cancellation_propagate_without_retry(self):
        for failure in (RuntimeError("host connection closed"), OSError("pipe closed"),
                        asyncio.CancelledError()):
            for call in (lambda: mcp.list_tools("docs"), lambda: mcp.call_tool("docs", "search")):
                host = AsyncMock(side_effect=failure)
                with self.subTest(failure=type(failure).__name__):
                    with patch.object(repl, "host_request", host):
                        with self.assertRaises(type(failure)) as error:
                            await call()
                    self.assertIs(error.exception, failure)
                    host.assert_awaited_once()

    async def test_missing_host_fails_closed(self):
        for call in (lambda: mcp.list_tools("docs"), lambda: mcp.call_tool("docs", "search")):
            with self.assertRaisesRegex(RuntimeError, "not serving"):
                await call()

    async def test_malformed_host_response_fails(self):
        for reply in (None, [], "invalid", {"status": "ok"},
                      {"status": "ok", "result": None}, {"status": "ok", "result": []}):
            for call in (lambda: mcp.list_tools("docs"), lambda: mcp.call_tool("docs", "search")):
                with self.subTest(reply=reply):
                    with patch.object(repl, "host_request", AsyncMock(return_value=reply)):
                        with self.assertRaises(RuntimeError):
                            await call()
        for result in ({}, {"tools": None}, {"tools": {}}):
            with patch.object(repl, "host_request", AsyncMock(return_value={"status": "ok", "result": result})):
                with self.assertRaisesRegex(RuntimeError, "result.tools"):
                    await mcp.list_tools("docs")

    async def test_invalid_inputs_never_reach_host(self):
        calls = [lambda: mcp.list_tools(""), lambda: mcp.list_tools(None),
                 lambda: mcp.call_tool("", "search"), lambda: mcp.call_tool("docs", ""),
                 lambda: mcp.call_tool("docs", None), lambda: mcp.call_tool("docs", "search", [])]
        host = AsyncMock()
        with patch.object(repl, "host_request", host):
            for call in calls:
                with self.assertRaises(TypeError):
                    await call()
        host.assert_not_awaited()

    async def test_repl_shutdown_keeps_local_child_cleanup_with_facade_loaded(self):
        queue = asyncio.Queue()
        await queue.put({"type": "shutdown", "id": "shutdown-1"})
        with (
            patch.object(repl.signal, "signal"),
            patch.object(repl, "_kill_live_handles") as cleanup,
            patch.object(repl, "_send") as send,
        ):
            await repl._serve(queue, {"mcp": mcp})
        cleanup.assert_called_once_with()
        send.assert_called_once_with({"event": "done", "id": "shutdown-1", "status": "ok"})

    def test_generic_facade_has_no_local_transport_or_lifecycle(self):
        self.assertEqual(mcp.__all__, ["call_tool", "list_tools"])
        for name in ("_Registry", "_Generation", "_registry", "_dispatch", "_StderrTail",
                     "reload", "close", "McpStartupError"):
            self.assertFalse(hasattr(mcp, name), name)


if __name__ == "__main__":
    unittest.main()

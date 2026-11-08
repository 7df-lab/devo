"""Authored integration lifecycle through the real typed host reply adapter.

Uses temporary synthetic grants only; never touches a developer's auth store.
"""

import asyncio
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import time
import traceback
from types import SimpleNamespace
import unittest
from unittest.mock import AsyncMock, patch

from rlm import mcp_base, repl


class Linear(mcp_base.McpIntegration):
    server = "linear"
    url = "https://mcp.linear.app/mcp"


class ConnectionLifecycleTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.temp = TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name)
        self.home_patch = patch.object(mcp_base, "_agent_dir", return_value=self.home)
        self.home_patch.start()
        self.addCleanup(self.home_patch.stop)
        self.integration = Linear()

    def write_grant(self, **overrides):
        grant = {"type": "oauth", "endpoint": Linear.url, "access": "synthetic-access",
                 "refresh": "synthetic-refresh", "expires": (time.time() - 60) * 1000}
        grant.update(overrides)
        (self.home / "auth.json").write_text(json.dumps({"mcp:linear": grant}))

    async def test_missing_grant_is_not_enabled_and_never_refreshes(self):
        host = AsyncMock()
        with patch.object(repl, "host_request", host):
            with self.assertRaises(mcp_base.NotEnabled):
                await self.integration._resolve_token()
        host.assert_not_awaited()

    async def test_expired_grant_refreshes_once_for_concurrent_calls(self):
        self.write_grant()

        async def rotate(request):
            self.assertEqual(request, {"type": "mcp.refresh", "server": "linear"})
            await asyncio.sleep(0)
            self.write_grant(access="rotated-synthetic-access", expires=(time.time() + 3600) * 1000)
            return {"status": "ok", "result": {}}

        host = AsyncMock(side_effect=rotate)
        with patch.object(repl, "host_request", host):
            results = await asyncio.gather(*(self.integration._resolve_token() for _ in range(5)))
        self.assertEqual(results, ["rotated-synthetic-access"] * 5)
        host.assert_awaited_once()

    async def test_noop_refresh_is_not_misreported_as_missing_login(self):
        self.write_grant()
        host = AsyncMock(return_value={"status": "ok", "result": {"servers": []}})
        with patch.object(repl, "host_request", host):
            with self.assertRaisesRegex(RuntimeError, "without usable credentials") as caught:
                await self.integration._resolve_token()
        self.assertNotIsInstance(caught.exception, mcp_base.NotEnabled)
        host.assert_awaited_once()

    async def test_native_refresh_error_is_actionable_and_redacted(self):
        self.write_grant()
        host = AsyncMock(return_value={"status": "error", "error": (
            "This Rust host cannot refresh authored integration credentials in auth.json. "
            "synthetic-secret-token"
        )})
        with patch.object(repl, "host_request", host):
            try:
                await self.integration._resolve_token()
            except RuntimeError as error:
                rendered = "".join(traceback.format_exception(error))
                self.assertIn("Rust host", rendered)
                self.assertNotIn("synthetic-secret-token", rendered)
            else:
                self.fail("host rejection must fail")
        host.assert_awaited_once()

    async def test_generic_refresh_failure_does_not_misidentify_the_host(self):
        self.write_grant()
        host = AsyncMock(return_value={"status": "error", "error": "temporary provider failure"})
        with patch.object(repl, "host_request", host):
            with self.assertRaises(RuntimeError) as caught:
                await self.integration._resolve_token()
        self.assertIn("Failed to refresh", str(caught.exception))
        self.assertNotIn("Rust host", str(caught.exception))

    async def test_endpoint_mismatch_never_refreshes_or_transmits(self):
        self.write_grant(endpoint="https://another.example/mcp", expires=(time.time() + 3600) * 1000)
        host = AsyncMock()
        with patch.object(repl, "host_request", host):
            with self.assertRaisesRegex(RuntimeError, "another endpoint"):
                await self.integration._resolve_token()
        host.assert_not_awaited()

    async def test_bad_expiry_and_access_are_not_treated_as_fresh(self):
        for expires in (True, float("inf"), float("nan"), "9999999999999", 10**1000, None):
            with self.subTest(expires=expires):
                self.write_grant(expires=expires)
                self.assertIsNone(self.integration._token())
        self.write_grant(access={"unexpected": "object"}, expires=(time.time() + 3600) * 1000)
        self.assertIsNone(self.integration._token())

    async def test_cancelled_refresh_does_not_retry_or_poison_lock(self):
        self.write_grant()
        host = AsyncMock(side_effect=asyncio.CancelledError())
        with patch.object(repl, "host_request", host):
            with self.assertRaises(asyncio.CancelledError):
                await self.integration._resolve_token()
        self.assertFalse(self.integration._refresh_lock.locked())
        host.assert_awaited_once()

    async def test_explicit_discovery_rechecks_connection_and_clears_stale_tools(self):
        session = SimpleNamespace(list_tools=AsyncMock(return_value=SimpleNamespace(tools=[
            SimpleNamespace(name="list_issues", description="List", input_schema={})
        ])))
        with patch.object(self.integration, "_open_session", AsyncMock(return_value=session)):
            self.assertEqual(await self.integration.list_tools(), [
                {"name": "list_issues", "description": "List", "inputSchema": {}}
            ])
            session.list_tools.return_value = SimpleNamespace(tools=[])
            self.assertEqual(await self.integration.list_tools(), [])
            session.list_tools.side_effect = RuntimeError("disconnected")
            with self.assertRaisesRegex(RuntimeError, "disconnected"):
                await self.integration.list_tools()
        self.assertIsNone(self.integration._tools)
        self.assertEqual(session.list_tools.await_count, 3)

    async def test_mutating_call_failure_is_never_replayed(self):
        session = SimpleNamespace(call_tool=AsyncMock(side_effect=RuntimeError("connection lost")))
        with patch.object(self.integration, "_open_session", AsyncMock(return_value=session)):
            with self.assertRaisesRegex(RuntimeError, "connection lost"):
                await self.integration.call_tool("create_issue", {"title": "Test"})
        session.call_tool.assert_awaited_once_with("create_issue", {"title": "Test"})


if __name__ == "__main__":
    unittest.main()

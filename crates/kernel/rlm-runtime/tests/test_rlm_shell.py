import unittest
from unittest.mock import patch
import rlm


class ShellApprovalTests(unittest.IsolatedAsyncioTestCase):
    async def test_namespace_and_module_forward_permission_request(self):
        calls = []
        async def host(kind, payload):
            calls.append((kind, payload))
            return {"status": "ok", "stdout": "probe"}
        with patch.object(rlm, "host_request", new=host):
            for helper in [rlm.shell, rlm.rlm.shell]:
                self.assertEqual(await helper(
                    "echo probe", sandbox_permissions="require_escalated",
                    justification="May I run the probe?", timeout_ms=1000,
                ), {"status": "ok", "stdout": "probe"})
        self.assertEqual(calls, [("bash", {
            "command": "echo probe", "sandbox_permissions": "require_escalated",
            "justification": "May I run the probe?", "timeout_ms": 1000,
        })] * 2)

    async def test_narrow_grant_and_prefix_are_preserved(self):
        calls = []
        async def host(kind, payload):
            calls.append((kind, payload))
            return {"status": "ok"}
        with patch.object(rlm, "host_request", new=host):
            await rlm.shell("curl example.test", sandbox_permissions="with_additional_permissions",
                additional_permissions={"network": {"enabled": True}}, prefix_rule=["curl"])
        self.assertEqual(calls, [("bash", {"command": "curl example.test",
            "sandbox_permissions": "with_additional_permissions",
            "additional_permissions": {"network": {"enabled": True}}, "prefix_rule": ["curl"]})])

    async def test_denial_propagates_without_retrying_locally(self):
        calls = []
        async def host(kind, payload):
            calls.append(kind)
            raise RuntimeError("declined by user")
        with patch.object(rlm, "host_request", new=host):
            with self.assertRaisesRegex(RuntimeError, "declined by user"):
                await rlm.shell("echo probe", sandbox_permissions="require_escalated")
        self.assertEqual(calls, ["bash"])

    async def test_empty_command_never_calls_host(self):
        async def host(kind, payload):
            self.fail("invalid command reached the host")
        with patch.object(rlm, "host_request", new=host):
            with self.assertRaises(ValueError):
                await rlm.shell(" ")

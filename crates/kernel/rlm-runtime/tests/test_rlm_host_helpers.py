import os
import unittest
from importlib import import_module
from pathlib import Path
from unittest.mock import patch

import rlm

harness = import_module("rlm.harness")
mcp_base = import_module("rlm.mcp_base")


class AgentDirectoryTests(unittest.TestCase):
    def test_harness_and_mcp_defaults_use_devo_config_dir(self):
        env = {
            "DEVO_CODING_AGENT_DIR": "",
            "DEVO_HOME": "",
            "PI_CODING_AGENT_DIR": "",
        }
        expected = Path("/tmp/devo-agent-dir-test") / ".devo"
        with patch.dict(os.environ, env), patch.object(
            Path, "home", return_value=Path("/tmp/devo-agent-dir-test")
        ):
            self.assertEqual(harness._agent_dir(), expected)
            self.assertEqual(mcp_base._agent_dir(), expected)


class RlmHostHelperTests(unittest.IsolatedAsyncioTestCase):
    async def test_helpers_use_typed_host_requests(self):
        calls = []

        async def fake_host_request(request_type, payload=None):
            calls.append((request_type, payload))
            return {"accepted": True}

        with patch.object(rlm, "host_request", new=fake_host_request):
            self.assertEqual(await rlm.rlm.web_search("  Rust docs  "), {"accepted": True})
            self.assertEqual(
                await rlm.rlm.web_fetch(" https://example.test ", format="text", timeout=12),
                {"accepted": True},
            )
            self.assertEqual(
                await rlm.rlm.update_plan(
                    [{"step": "Inspect", "status": "in_progress"}],
                    explanation="Starting",
                ),
                {"accepted": True},
            )
            self.assertEqual(
                await rlm.rlm.request_user_input([{"id": "choice"}]),
                {"accepted": True},
            )

        self.assertEqual(
            calls,
            [
                ("web.search", {"query": "Rust docs"}),
                (
                    "web.fetch",
                    {"url": "https://example.test", "format": "text", "timeout": 12},
                ),
                (
                    "plan.update",
                    {
                        "plan": [{"step": "Inspect", "status": "in_progress"}],
                        "explanation": "Starting",
                    },
                ),
                ("question", {"questions": [{"id": "choice"}]}),
            ],
        )

    async def test_search_rejects_empty_query_before_host_request(self):
        async def fake_host_request(request_type, payload=None):
            self.fail(f"unexpected host request: {request_type} {payload}")

        with patch.object(rlm, "host_request", new=fake_host_request):
            with self.assertRaises(ValueError):
                await rlm.rlm.web_search("   ")


if __name__ == "__main__":
    unittest.main()

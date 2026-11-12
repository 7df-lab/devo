"""Exercise the bundled Python skill, leaving delivery policy to the host."""

import importlib.util
from pathlib import Path
import unittest
from unittest.mock import AsyncMock, patch

import rlm


skill_path = (
    Path(__file__).resolve().parents[3]
    / "skills/assets/bundled/agent-message/src/agent_message/__init__.py"
)
spec = importlib.util.spec_from_file_location("bundled_agent_message", skill_path)
agent_message = importlib.util.module_from_spec(spec)
spec.loader.exec_module(agent_message)


class AgentMessageTests(unittest.IsolatedAsyncioTestCase):
    async def test_self_send_uses_host_derived_identity_and_returns_admission(self):
        receipt = {"status": "ok", "deliveryStatus": "queued", "queueItemId": "queue-1"}
        host = AsyncMock(return_value=receipt)
        with patch.object(agent_message, "host_request", host), patch.object(rlm, "emit") as emit:
            self.assertEqual(
                await agent_message.send("check result", receiver_role="self"), receipt
            )
        host.assert_awaited_once_with("agent_message.send", {
            "message": "check result", "receiver_role": "self", "receiver_name": None,
        })
        self.assertEqual(emit.call_args.args[0], {
            "application/vnd.agent-message+json": {**receipt, "receiverRole": "self"},
            "text/plain": "Agent message queued",
        })

    async def test_self_cannot_supply_a_name_or_delivery_override(self):
        host = AsyncMock()
        with patch.object(agent_message, "host_request", host):
            with self.assertRaises(ValueError):
                await agent_message.send("check", receiver_role="self", receiver_name="other")
            with self.assertRaises(TypeError):
                await agent_message.send("check", receiver_role="self", delivery_mode="steer")
        host.assert_not_awaited()

    async def test_host_failure_does_not_emit_a_success_receipt(self):
        host = AsyncMock(side_effect=RuntimeError("session closed"))
        with patch.object(agent_message, "host_request", host), patch.object(rlm, "emit") as emit:
            with self.assertRaisesRegex(RuntimeError, "session closed"):
                await agent_message.send("check", receiver_role="self")
        emit.assert_not_called()

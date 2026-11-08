import unittest
from pathlib import Path
from unittest.mock import AsyncMock, patch
import rlm


class ControlsTests(unittest.IsolatedAsyncioTestCase):
    async def test_collect_mixed_targets_and_retained_partial_result(self):
        entry = dict(rlm_child_id="id", session_name="worker", session_dir="artifacts", status="running", settled=False, progress_note="testing")
        host = AsyncMock(return_value={"results": [entry]})
        handle = rlm.RLMSpawnHandle("id", "worker", Path("artifacts"), "model")
        with patch.object(rlm, "host_request", host):
            result = await rlm.rlm.collect([handle, "worker"], timeout_ms=25)
        self.assertEqual(result, [rlm.RLMChildResult("id", "worker", Path("artifacts"), "running", False, progress_note="testing")])
        host.assert_awaited_once_with("rlm.collect", {"targets": ["id", "worker"], "timeout_ms": 25})

    async def test_invalid_wait_and_target_do_not_call_host(self):
        host = AsyncMock()
        with patch.object(rlm, "host_request", host):
            for timeout in (True, None, "1", 1.0):
                with self.assertRaises(TypeError):
                    await rlm.collect(timeout_ms=timeout)
            for timeout in (-1, 300001):
                with self.assertRaises(ValueError):
                    await rlm.collect(timeout_ms=timeout)
            for targets in (False, [False], {}):
                with self.assertRaises(TypeError):
                    await rlm.collect(targets)
            with self.assertRaises(ValueError):
                await rlm.collect([" "])
        host.assert_not_awaited()

    async def test_progress_utf16_boundary_and_throttle(self):
        host = AsyncMock(return_value={"accepted": False, "retry_after_ms": 9000})
        with patch.object(rlm, "host_request", host):
            self.assertEqual(await rlm.rlm.progress_note(" " + "🎉" * 256 + " "), rlm.RLMProgressNoteResult(False, 9000))
            with self.assertRaises(ValueError):
                await rlm.progress_note("🎉" * 257)
            with self.assertRaises(ValueError):
                await rlm.progress_note(" ")
        host.assert_awaited_once_with("rlm.progress.note", {"message": "🎉" * 256})

    async def test_end_turn_and_host_failure(self):
        host = AsyncMock(return_value={"ended": True})
        with patch.object(rlm, "host_request", host):
            self.assertEqual(await rlm.rlm.end_turn(), {"ended": True})
        host.assert_awaited_once_with("rlm.end_turn", {})
        with patch.object(rlm, "host_request", AsyncMock(side_effect=RuntimeError("inactive"))):
            with self.assertRaisesRegex(RuntimeError, "inactive"):
                await rlm.end_turn()

    async def test_invalid_host_payload(self):
        for payload in ({}, {"results": [None]}, {"results": [{}]}):
            with patch.object(rlm, "host_request", AsyncMock(return_value=payload)):
                with self.assertRaises(RuntimeError):
                    await rlm.collect()

    async def test_create_session_validates_options_before_host(self):
        host = AsyncMock()
        with patch.object(rlm, "host_request", host):
            for kwargs in ({"prompt": " "}, {"prompt": "task", "cwd": " "}, {"prompt": "task", "thinking": " high "}):
                with self.assertRaises(ValueError):
                    await rlm.create_session(**kwargs)
            for field in ("name", "model", "thinking", "cwd"):
                with self.assertRaises(TypeError):
                    await rlm.create_session("task", **{field: True})
        host.assert_not_awaited()

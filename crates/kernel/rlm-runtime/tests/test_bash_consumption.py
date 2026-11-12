"""Foreground delivery is acknowledged before a cell can release its turn."""
import asyncio
import unittest
from importlib import import_module
from unittest.mock import patch

import rlm
from rlm import repl

bash_module = import_module("rlm.bash")


def terminal(error=None):
    return {"handle_id": "interactive-1", "command": "test", "mode": "interactive",
            "ready": True, "running": False, "exit_code": None if error else 0,
            "error": error, "output": "retained", "duration": 1.0}


def ack(disposition="consumed"):
    return {"accepted": True, "disposition": disposition}


class BashConsumptionTests(unittest.IsolatedAsyncioTestCase):
    async def test_result_wait_blocks_on_ack_and_repeated_wait_is_idempotent(self):
        entered, release = asyncio.Event(), asyncio.Event()
        calls = []
        async def host(kind, payload):
            calls.append((kind, payload))
            if kind == "bash.consumed":
                entered.set()
                await release.wait()
                return ack()
            return terminal()
        with patch.object(rlm, "host_request", host), \
             patch.object(bash_module, "_live_cell_owner", side_effect=asyncio.current_task):
            h = rlm.bash("test", mode="interactive")
            waiter = asyncio.ensure_future(h)
            await entered.wait()
            self.assertFalse(waiter.done())
            self.assertFalse(h._consumed)
            release.set()
            result = await waiter
            self.assertEqual(await h, result)
            self.assertEqual(result, rlm.BashResult(0, "retained", 1.0))
        self.assertEqual(calls, [("bash.start", {"command": "test", "mode": "interactive",
            "workdir": __import__("os").getcwd(), "sandbox_permissions": "use_default"}),
            ("bash.consumed", {"handle_id": "interactive-1", "command": "test", "managed": True})])

    async def test_terminal_error_acknowledged_before_raising_and_bytes_remain_readable(self):
        calls = []
        async def host(kind, payload):
            calls.append(kind)
            if kind == "bash.consumed":
                return ack()
            if kind == "bash.read":
                return {"data": "eA==", "next_cursor": 1, "eof": True, "truncated": False}
            return terminal("command cancelled")
        with patch.object(rlm, "host_request", host), \
             patch.object(bash_module, "_live_cell_owner", side_effect=asyncio.current_task):
            h = await rlm.bash.attach("interactive-1")
            with self.assertRaisesRegex(RuntimeError, "command cancelled"):
                await h.wait()
            self.assertTrue(h._consumed)
            self.assertEqual((await h.read()).data, b"x")
        self.assertEqual(calls, ["bash.attach", "bash.consumed", "bash.read"])

    async def test_ack_failure_retains_result_and_next_await_retries(self):
        attempts = 0
        async def host(kind, payload):
            nonlocal attempts
            if kind == "bash.consumed":
                attempts += 1
                if attempts == 1:
                    raise RuntimeError("ack response lost")
                return ack("already_consumed")
            return terminal()
        with patch.object(rlm, "host_request", host), \
             patch.object(bash_module, "_live_cell_owner", side_effect=asyncio.current_task):
            h = rlm.bash("test", mode="interactive")
            with self.assertRaisesRegex(RuntimeError, "ack response lost"):
                await h
            self.assertFalse(h._consumed)
            self.assertEqual(await h, rlm.BashResult(0, "retained", 1.0))
            self.assertEqual(h._consumption_status, "already_consumed")
            self.assertIsNone(h._error)
        self.assertEqual(attempts, 2)

    async def test_already_delivered_is_success_without_claiming_retraction(self):
        async def host(kind, payload):
            return ack("already_delivered") if kind == "bash.consumed" else terminal()
        with patch.object(rlm, "host_request", host), \
             patch.object(bash_module, "_live_cell_owner", side_effect=asyncio.current_task):
            h = rlm.bash("test", mode="interactive")
            self.assertEqual(await h, rlm.BashResult(0, "retained", 1.0))
            self.assertEqual(h._consumption_status, "already_delivered")

    async def test_partial_and_cached_reads_never_consume_completion(self):
        calls = []
        async def host(kind, payload):
            calls.append(kind)
            if kind == "bash.read":
                return {"data": "eA==", "next_cursor": 1, "eof": True, "truncated": False}
            return terminal()
        with patch.object(rlm, "host_request", host), \
             patch.object(bash_module, "_live_cell_owner", side_effect=asyncio.current_task):
            h = await rlm.bash.attach("interactive-1")
            await h.read()
            h.output()
            h.tail()
            h.poll()
            await asyncio.sleep(0)
            self.assertFalse(h._consumed)
        self.assertEqual(calls, ["bash.attach", "bash.read"])

    async def test_wait_timeout_does_not_acknowledge_or_cancel_process(self):
        finish = asyncio.Event()
        calls = []
        async def host(kind, payload):
            calls.append(kind)
            if kind == "bash.start":
                return {**terminal(), "running": True, "exit_code": None}
            await finish.wait()
            return terminal()
        with patch.object(rlm, "host_request", host), \
             patch.object(bash_module, "_live_cell_owner", side_effect=asyncio.current_task), \
             patch.object(bash_module, "_POLL_INTERVAL", 0):
            h = rlm.bash("test", mode="interactive")
            with self.assertRaises(TimeoutError):
                await h.wait(timeout_ms=1)
            self.assertFalse(h._consumed)
            self.assertTrue(h.running)
            finish.set()
            await h._task
        self.assertEqual(calls, ["bash.start", "bash.poll"])

    async def test_detached_awaiter_does_not_consume_for_the_live_cell(self):
        calls = []
        owner = asyncio.current_task()
        async def host(kind, payload):
            calls.append(kind)
            return terminal()
        with patch.object(rlm, "host_request", host), \
             patch.object(bash_module, "_live_cell_owner", return_value=owner):
            h = rlm.bash("test", mode="interactive")
            detached = asyncio.ensure_future(h)
            # Yield without awaiting detached: current cell does not consume it.
            while not detached.done():
                await asyncio.sleep(0)
            self.assertEqual(detached.result(), rlm.BashResult(0, "retained", 1.0))
            self.assertFalse(h._consumed)
        self.assertEqual(calls, ["bash.start"])

    async def test_real_cell_done_is_emitted_only_after_consumption_ack(self):
        frames, calls = [], []
        async def host(kind, payload):
            calls.append(kind)
            if kind == "bash.consumed":
                await asyncio.sleep(0)
                self.assertFalse(any(f["event"] == "done" for f in frames))
                return ack()
            return terminal()
        namespace = {"bash": rlm.bash}
        with patch.object(rlm, "host_request", host), \
             patch.object(repl, "_loop", asyncio.get_running_loop()), \
             patch.object(repl, "_send", side_effect=frames.append), \
             patch.object(repl, "_drain_output"), patch.object(repl, "emit"), \
             patch.object(repl, "is_active", return_value=True):
            await repl._handle_execute({"id": "foreground", "code":
                "h = bash('test', mode='interactive')\nr = await h.wait(timeout_ms=1000)"}, namespace)
            await namespace["h"]._notice_task
        self.assertEqual(calls, ["bash.start", "bash.consumed"])
        self.assertEqual(frames[-1], {"event": "done", "id": "foreground", "status": "ok"})

    async def test_invalid_ack_is_not_cached_as_consumed(self):
        async def host(kind, payload):
            return {} if kind == "bash.consumed" else terminal()
        with patch.object(rlm, "host_request", host), \
             patch.object(bash_module, "_live_cell_owner", side_effect=asyncio.current_task):
            h = rlm.bash("test", mode="interactive")
            with self.assertRaisesRegex(RuntimeError, "invalid consumption"):
                await h
            self.assertFalse(h._consumed)

    async def test_concurrent_foreground_waits_share_ack(self):
        attempts = 0
        async def host(kind, payload):
            nonlocal attempts
            if kind == "bash.consumed":
                attempts += 1
                await asyncio.sleep(0)
                return ack()
            return terminal()
        with patch.object(rlm, "host_request", host), \
             patch.object(bash_module, "_live_cell_owner", return_value=asyncio.current_task()):
            h = rlm.bash("test", mode="interactive")
            results = await asyncio.gather(h.wait(), h.wait())
            self.assertEqual(results, [rlm.BashResult(0, "retained", 1.0)] * 2)
        self.assertEqual(attempts, 1)

    async def test_cancellation_during_ack_can_retry_without_killing_or_losing_result(self):
        entered = asyncio.Event()
        attempts = 0
        calls = []
        async def host(kind, payload):
            nonlocal attempts
            calls.append(kind)
            if kind == "bash.consumed":
                attempts += 1
                if attempts == 1:
                    entered.set()
                    await asyncio.Event().wait()
                return ack("already_consumed")
            return terminal()
        with patch.object(rlm, "host_request", host), \
             patch.object(bash_module, "_live_cell_owner", side_effect=asyncio.current_task):
            h = rlm.bash("test", mode="interactive")
            waiter = asyncio.ensure_future(h)
            await entered.wait()
            waiter.cancel()
            with self.assertRaises(asyncio.CancelledError):
                await waiter
            self.assertFalse(h._consumed)
            self.assertEqual(await h, rlm.BashResult(0, "retained", 1.0))
        self.assertEqual(calls, ["bash.start", "bash.consumed", "bash.consumed"])

    async def test_process_wait_timeout_does_not_cancel_a_terminal_ack(self):
        async def host(kind, payload):
            if kind == "bash.consumed":
                # Acknowledgement has its own host transport lifetime, separate
                # from the requested process wait timeout.
                await asyncio.sleep(0.01)
                return ack()
            return terminal()
        with patch.object(rlm, "host_request", host), \
             patch.object(bash_module, "_live_cell_owner", side_effect=asyncio.current_task):
            h = rlm.bash("test", mode="interactive")
            await h._task
            self.assertEqual(await h.wait(timeout_ms=1), rlm.BashResult(0, "retained", 1.0))
            self.assertTrue(h._consumed)

    async def test_real_cell_terminal_error_ack_precedes_error_frame(self):
        frames, calls = [], []
        async def host(kind, payload):
            calls.append(kind)
            if kind == "bash.consumed":
                self.assertEqual(frames, [])
                await asyncio.sleep(0)
                return ack()
            return terminal("command timed out")
        namespace = {"bash": rlm.bash}
        with patch.object(rlm, "host_request", host), \
             patch.object(repl, "_loop", asyncio.get_running_loop()), \
             patch.object(repl, "_send", side_effect=frames.append), \
             patch.object(repl, "_drain_output"), patch.object(repl, "emit"), \
             patch.object(repl, "is_active", return_value=True), patch("sys.stderr.write"):
            await repl._handle_execute({"id": "error-result", "code":
                "h = bash('test', mode='interactive')\nawait h"}, namespace)
            await namespace["h"]._notice_task
        self.assertEqual(calls, ["bash.start", "bash.consumed"])
        self.assertEqual(frames[-1], {"event": "done", "id": "error-result", "status": "error"})
        self.assertIn("command timed out", str(frames[0]))

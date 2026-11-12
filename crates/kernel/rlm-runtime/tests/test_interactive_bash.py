import asyncio
import base64
import unittest
from importlib import import_module
from unittest.mock import patch

import rlm

bash_module = import_module("rlm.bash")


def snapshot(*, mode="interactive", ready=True, running=True, code=None):
    return {"handle_id": "interactive-1", "command": "test command", "mode": mode,
            "ready": ready, "running": running, "exit_code": code,
            "output": "", "duration": 1.0}


class InteractiveBashTests(unittest.IsolatedAsyncioTestCase):
    async def test_ready_waits_for_spawn_not_only_admission(self):
        admitted, spawn, finish = asyncio.Event(), asyncio.Event(), asyncio.Event()
        async def host(kind, payload):
            if kind == "bash.start":
                admitted.set()
                return snapshot(ready=False)
            if not spawn.is_set():
                await spawn.wait()
                return snapshot()
            await finish.wait()
            return snapshot(running=False, code=0)
        with patch.object(rlm, "host_request", host), patch.object(bash_module, "_POLL_INTERVAL", 0):
            h = rlm.bash("test command", mode="interactive")
            ready = asyncio.create_task(h.ready())
            await admitted.wait()
            await asyncio.sleep(0)
            self.assertFalse(ready.done())
            spawn.set()
            self.assertIs(await ready, h)
            finish.set()
            await h

    async def test_binary_write_cursor_read_and_eof(self):
        finish = asyncio.Event()
        calls = []
        raw = b"\xff\x00\xe4\xbd"
        async def host(kind, payload):
            calls.append((kind, payload))
            if kind == "bash.start":
                return snapshot()
            if kind == "bash.poll":
                await finish.wait()
                return snapshot(running=False, code=0)
            if kind == "bash.read":
                return {"data": base64.b64encode(raw).decode(), "next_cursor": 12,
                        "truncated": True, "eof": False}
            return {}
        with patch.object(rlm, "host_request", host):
            h = rlm.bash("test command", mode="interactive")
            await h.ready()
            await h.write(raw)
            chunk = await h.read(cursor=8, max_bytes=4, wait_ms=10)
            self.assertEqual(chunk, rlm.BashRead(raw, 12, True, False))
            self.assertEqual(chunk.text, raw.decode(errors="replace"))
            await h.close_stdin()
            finish.set()
            await h
        self.assertIn(("bash.write", {"handle_id": "interactive-1", "data": "/wDkvQ=="}), calls)
        self.assertIn(("bash.close_stdin", {"handle_id": "interactive-1"}), calls)
        self.assertNotIn("bash.forget", [kind for kind, _ in calls])

    async def test_wait_timeout_and_cancelled_await_do_not_kill_interactive(self):
        finish = asyncio.Event()
        calls = []
        async def host(kind, payload):
            calls.append(kind)
            if kind == "bash.start":
                return snapshot()
            await finish.wait()
            return snapshot(running=False, code=0)
        with patch.object(rlm, "host_request", host):
            h = rlm.bash("test command", mode="interactive")
            with self.assertRaises(TimeoutError):
                await h.wait(timeout_ms=1)
            await h.ready()
            waiter = asyncio.ensure_future(h)
            await asyncio.sleep(0)
            waiter.cancel()
            with self.assertRaises(asyncio.CancelledError):
                await waiter
            self.assertTrue(h.running)
            finish.set()
            self.assertEqual(await h.wait(), rlm.BashResult(0, "", 1.0))
        self.assertNotIn("bash.kill", calls)

    async def test_poll_disconnect_does_not_kill_interactive(self):
        calls = []
        async def host(kind, payload):
            calls.append(kind)
            if kind == "bash.start":
                return snapshot()
            raise RuntimeError("disconnected")
        with patch.object(rlm, "host_request", host), patch.object(bash_module, "_POLL_INTERVAL", 0):
            h = rlm.bash("test command", mode="interactive")
            with self.assertRaisesRegex(RuntimeError, "disconnected"):
                await h
            with self.assertRaisesRegex(RuntimeError, "disconnected"):
                await h.ready()
        self.assertEqual(calls, ["bash.start", "bash.poll"])

    async def test_spawn_failure_unblocks_ready(self):
        async def host(kind, payload):
            return {**snapshot(ready=False, running=False), "error": "spawn failed"}
        with patch.object(rlm, "host_request", host):
            h = rlm.bash("test command", mode="interactive")
            with self.assertRaisesRegex(RuntimeError, "spawn failed"):
                await h.ready()

    async def test_attach_does_not_start_a_process(self):
        calls = []
        state = snapshot(running=False, code=0)
        async def host(kind, payload):
            calls.append((kind, payload))
            if kind == "bash.list":
                return {"handles": [state]}
            return state
        with patch.object(rlm, "host_request", host):
            self.assertEqual(await rlm.bash.list(), [state])
            h = await rlm.bash.attach("interactive-1")
            self.assertEqual(await h, rlm.BashResult(0, "", 1.0))
        self.assertEqual(calls, [("bash.list", {}), ("bash.attach", {"handle_id": "interactive-1"})])

    async def test_pty_resize_rejects_half_close(self):
        calls = []
        async def host(kind, payload):
            calls.append((kind, payload))
            return snapshot(mode="pty", running=False, code=0)
        with patch.object(rlm, "host_request", host):
            h = rlm.bash("test command", mode="pty")
            await h.ready()
            await h.resize(cols=80, rows=24)
            with self.assertRaisesRegex(ValueError, "half-close"):
                await h.close_stdin()
            await h
        self.assertIn(("bash.resize", {"handle_id": "interactive-1", "cols": 80, "rows": 24}), calls)
        self.assertNotIn("bash.close_stdin", [kind for kind, _ in calls])

    async def test_invalid_inputs_do_not_call_host(self):
        async def host(kind, payload):
            return snapshot(running=False, code=0)
        with patch.object(rlm, "host_request", host):
            with self.assertRaises(ValueError):
                rlm.bash("test", mode="unknown")
            h = rlm.bash("test", mode="interactive")
            await h
            for kwargs in ({"cursor": -1}, {"cursor": True}, {"max_bytes": 0},
                           {"max_bytes": 65537}, {"wait_ms": 30001}):
                with self.assertRaises(ValueError):
                    await h.read(**kwargs)
            with self.assertRaises(ValueError):
                await h.write(b"x" * 65537)
            with self.assertRaises(TypeError):
                await h.write(1)
            with self.assertRaises(ValueError):
                await h.resize(cols=0, rows=24)

    async def test_malformed_read_is_rejected(self):
        async def host(kind, payload):
            if kind == "bash.read":
                return {"data": "%%%", "next_cursor": 1, "truncated": False, "eof": True}
            return snapshot(running=False, code=0)
        with patch.object(rlm, "host_request", host):
            h = rlm.bash("test", mode="interactive")
            await h
            with self.assertRaisesRegex(RuntimeError, "invalid read"):
                await h.read()

    async def test_interactive_completion_uses_host_not_duplicate_python_notice(self):
        from rlm import repl
        finish = asyncio.Event()
        calls, displays = [], []
        async def host(kind, payload):
            calls.append(kind)
            return snapshot(running=False, code=0)
        with patch.object(rlm, "host_request", host), patch.object(repl, "emit", side_effect=displays.append), \
             patch.object(repl, "is_active", return_value=True), \
             patch.object(bash_module, "_current_cell_completion_context", return_value=(finish, None)):
            h = rlm.bash("test", mode="interactive")
            await h._task
            finish.set()
            await h._notice_task
        self.assertEqual(calls, ["bash.start"])
        self.assertEqual([d["application/vnd.bash-activity+json"]["active"] for d in displays], [True, False])

    async def test_send_line_and_terminate_ack(self):
        killed = asyncio.Event()
        calls = []
        async def host(kind, payload):
            calls.append((kind, payload))
            if kind == "bash.kill":
                killed.set()
                return snapshot()
            if kind == "bash.poll":
                await killed.wait()
                return snapshot(running=False, code=130)
            return snapshot()
        with patch.object(rlm, "host_request", host):
            h = rlm.bash("test", mode="interactive")
            await h.send_line("hello")
            await h.terminate()
            self.assertTrue(killed.is_set())
            self.assertEqual((await h).exit_code, 130)
        self.assertIn(("bash.write", {"handle_id": "interactive-1", "data": "aGVsbG8K"}), calls)

    async def test_idle_timeout_is_explicit_and_interactive_only(self):
        calls = []
        async def host(kind, payload):
            calls.append((kind, payload))
            return snapshot(running=False, code=0)
        with patch.object(rlm, "host_request", host):
            for kwargs in ({"idle_timeout_ms": 1}, {"mode": "interactive", "idle_timeout_ms": 0},
                           {"mode": "interactive", "idle_timeout_ms": True}):
                with self.assertRaises(ValueError):
                    rlm.bash("test", **kwargs)
            h = rlm.bash("test", mode="interactive", idle_timeout_ms=1000, timeout_ms=5000)
            await h
        self.assertEqual(len(calls), 1)
        self.assertEqual((calls[0][1]["idle_timeout_ms"], calls[0][1]["timeout_ms"]), (1000, 5000))

    async def test_read_preserves_stream_segments(self):
        async def host(kind, payload):
            if kind == "bash.read":
                return {"data": "b3V0ZXJy", "next_cursor": 6, "truncated": False, "eof": True,
                        "chunks": [{"stream": "stdout", "data_base64": "b3V0"},
                                   {"stream": "stderr", "data_base64": "ZXJy"}]}
            return snapshot(running=False, code=0)
        with patch.object(rlm, "host_request", host):
            h = rlm.bash("test", mode="interactive")
            await h
            self.assertEqual(await h.read(), rlm.BashRead(b"outerr", 6, False, True,
                (rlm.BashChunk("stdout", b"out"), rlm.BashChunk("stderr", b"err"))))

    async def test_cancelled_terminal_record_can_attach_and_read_but_wait_and_write_fail(self):
        calls = []
        state = {**snapshot(running=False), "error": "command cancelled", "output": "diagnostic"}
        async def host(kind, payload):
            calls.append(kind)
            if kind == "bash.read":
                return {"data": "/wBh", "next_cursor": 3, "truncated": False, "eof": True}
            return state
        with patch.object(rlm, "host_request", host):
            h = await rlm.bash.attach("interactive-1")
            self.assertEqual(await h.read(), rlm.BashRead(b"\xff\x00a", 3, False, True))
            with self.assertRaisesRegex(RuntimeError, "command cancelled"):
                await h.wait()
            with self.assertRaisesRegex(RuntimeError, "command cancelled"):
                await h.write("must not write")
        self.assertEqual(calls, ["bash.attach", "bash.read"])

    async def test_existing_handle_can_read_terminal_timeout_bytes(self):
        state = {**snapshot(running=False), "error": "command timed out"}
        calls = []
        async def host(kind, payload):
            calls.append(kind)
            if kind == "bash.start":
                return snapshot()
            if kind == "bash.read":
                return {"data": "bGFzdA==", "next_cursor": 4, "truncated": False, "eof": True}
            return state
        with patch.object(rlm, "host_request", host), patch.object(bash_module, "_POLL_INTERVAL", 0):
            h = rlm.bash("test", mode="interactive")
            with self.assertRaisesRegex(RuntimeError, "timed out"):
                await h
            self.assertEqual(await h.read(), rlm.BashRead(b"last", 4, False, True))
        self.assertEqual(calls, ["bash.start", "bash.poll", "bash.read"])

    async def test_terminate_ack_is_independent_of_terminal_cancel_error_and_idempotent(self):
        cancelling = asyncio.Event()
        calls = []
        h = None
        async def host(kind, payload):
            calls.append(kind)
            if kind == "bash.start":
                return snapshot()
            if kind == "bash.poll":
                await cancelling.wait()
                return {**snapshot(running=False), "error": "command cancelled"}
            if kind == "bash.kill":
                cancelling.set()
                # Force poll to install the real terminal error before the
                # cancellation acknowledgement reaches terminate().
                await asyncio.gather(h._task, return_exceptions=True)
                return {"cancelled": True}
            return {}
        with patch.object(rlm, "host_request", host), patch.object(bash_module, "_POLL_INTERVAL", 0):
            h = rlm.bash("test", mode="interactive")
            await h.ready()
            await h.terminate()
            await h.terminate()
            with self.assertRaisesRegex(RuntimeError, "command cancelled"):
                await h.wait()
        self.assertEqual(calls.count("bash.kill"), 1)

    async def test_terminate_still_reports_control_failure(self):
        finish = asyncio.Event()
        async def host(kind, payload):
            if kind == "bash.start":
                return snapshot()
            if kind == "bash.kill":
                raise RuntimeError("cancel transport failed")
            await finish.wait()
            return snapshot(running=False, code=0)
        with patch.object(rlm, "host_request", host), patch.object(bash_module, "_POLL_INTERVAL", 0):
            h = rlm.bash("test", mode="interactive")
            await h.ready()
            with self.assertRaisesRegex(RuntimeError, "cancel transport failed"):
                await h.terminate()
            finish.set()
            await asyncio.gather(h._task, return_exceptions=True)

    async def test_terminate_still_reports_admission_failure(self):
        async def host(kind, payload):
            raise RuntimeError("approval denied")
        with patch.object(rlm, "host_request", host):
            h = rlm.bash("test", mode="interactive")
            with self.assertRaisesRegex(RuntimeError, "approval denied"):
                await h.terminate()

import asyncio
import os
import unittest
from importlib import import_module
from unittest.mock import patch

import rlm

bash_module = import_module("rlm.bash")


def snapshot(running=True, output="", code=None):
    return {"handle_id": "host-opaque-1", "running": running,
            "exit_code": code, "output": output, "duration": 0.25}


class HostBashTests(unittest.IsolatedAsyncioTestCase):
    async def test_pending_cached_state_and_completion(self):
        admitted = asyncio.Event()
        calls = []
        async def host(kind, payload):
            calls.append((kind, payload))
            if kind == "bash.start":
                await admitted.wait()
                return snapshot(output="first")
            return snapshot(False, "first\nlast\n", 0)
        with patch.object(rlm, "host_request", host), patch.object(bash_module, "_POLL_INTERVAL", 0):
            handle = rlm.bash("echo hello")
            self.assertEqual((handle.handle_id, handle.pid, handle.running, handle.poll(), handle.output()),
                             (None, None, True, None, ""))
            self.assertEqual(calls, [])
            admitted.set()
            result = await handle
            self.assertEqual(result, rlm.BashResult(0, "first\nlast\n", 0.25))
            self.assertEqual((handle.handle_id, handle.pid, handle.running, handle.poll(), handle.tail(1)),
                             ("host-opaque-1", None, False, result, "last"))
            self.assertEqual(await handle, result)
        self.assertEqual(calls, [("bash.start", {"command": "echo hello", "workdir": os.getcwd(),
                                                "sandbox_permissions": "use_default"}),
                                 ("bash.poll", {"handle_id": "host-opaque-1"}),
                                 ("bash.forget", {"handle_id": "host-opaque-1"})])

    async def test_authorization_arguments(self):
        calls = []
        async def host(kind, payload):
            calls.append((kind, payload))
            return snapshot(False, "", 0)
        with patch.object(rlm, "host_request", host):
            await rlm.bash("probe", workdir="child", sandbox_permissions="with_additional_permissions",
                           additional_permissions={"network": {"enabled": True}},
                           justification="network probe", prefix_rule=["probe"], timeout_ms=42)
        self.assertEqual(calls, [("bash.start", {"command": "probe", "workdir": "child",
            "sandbox_permissions": "with_additional_permissions", "additional_permissions": {"network": {"enabled": True}},
            "justification": "network probe", "prefix_rule": ["probe"], "timeout_ms": 42}),
            ("bash.forget", {"handle_id": "host-opaque-1"})])

    async def test_denial_has_no_local_fallback(self):
        async def host(kind, payload):
            raise RuntimeError("denied")
        with patch.object(rlm, "host_request", host), patch("subprocess.Popen") as spawn:
            handle = rlm.bash("echo forbidden")
            with self.assertRaisesRegex(RuntimeError, "denied"):
                await handle
            with self.assertRaisesRegex(RuntimeError, "denied"):
                handle.poll()
            self.assertFalse(handle.running)
            spawn.assert_not_called()

    async def test_missing_host_fails_closed(self):
        with self.assertRaisesRegex(RuntimeError, "not serving"):
            await rlm.bash("echo forbidden")

    async def test_cancel_pending_owned_await_cancels_after_admission(self):
        started, admit, killed = asyncio.Event(), asyncio.Event(), asyncio.Event()
        calls = []
        async def host(kind, payload):
            calls.append(kind)
            if kind == "bash.start":
                started.set()
                await admit.wait()
                return snapshot()
            if kind == "bash.kill":
                killed.set()
                return snapshot()
            await killed.wait()
            return snapshot(False, "stopped", 130)
        with patch.object(rlm, "host_request", host), patch.object(bash_module, "_POLL_INTERVAL", 0):
            handle = rlm.bash("long command")
            task = asyncio.ensure_future(handle)
            await started.wait()
            task.cancel()
            with self.assertRaises(asyncio.CancelledError):
                await task
            self.assertNotIn("bash.kill", calls)
            admit.set()
            await killed.wait()
            self.assertEqual(await handle, rlm.BashResult(130, "stopped", 0.25))

    async def test_cancel_background_await_does_not_kill(self):
        started, finish = asyncio.Event(), asyncio.Event()
        calls = []
        async def host(kind, payload):
            calls.append(kind)
            if kind == "bash.start":
                started.set()
                return snapshot()
            await finish.wait()
            return snapshot(False, "done", 0)
        with patch.object(rlm, "host_request", host), patch.object(bash_module, "_POLL_INTERVAL", 0):
            handle = rlm.bash("background")
            self.assertTrue(handle.running)
            task = asyncio.ensure_future(handle)
            await started.wait()
            task.cancel()
            with self.assertRaises(asyncio.CancelledError):
                await task
            finish.set()
            await handle
        self.assertNotIn("bash.kill", calls)

    async def test_kill_pending_handle(self):
        killed = asyncio.Event()
        async def host(kind, payload):
            if kind == "bash.kill":
                killed.set()
            if kind == "bash.poll":
                await killed.wait()
                return snapshot(False, "", 130)
            return snapshot()
        with patch.object(rlm, "host_request", host), patch.object(bash_module, "_POLL_INTERVAL", 0):
            handle = rlm.bash("background")
            handle.kill()
            handle.kill()
            self.assertEqual(await handle, rlm.BashResult(130, "", 0.25))

    async def test_background_notice_and_consumption_use_opaque_id(self):
        from rlm import repl
        cell_finished, notice = asyncio.Event(), asyncio.Event()
        calls, displays = [], []
        async def host(kind, payload):
            calls.append((kind, payload))
            if kind == "bash.completed":
                notice.set()
                return {}
            if kind == "bash.consumed":
                return {}
            return snapshot(False, "done", 0)
        with patch.object(rlm, "host_request", host), patch.object(repl, "is_active", return_value=True),              patch.object(repl, "emit", side_effect=displays.append),              patch.object(bash_module, "_current_cell_completion_context", return_value=(cell_finished, None)),              patch.object(bash_module, "_live_cell_owner", return_value=None):
            handle = rlm.bash("background")
            await handle._task
            self.assertEqual(len(calls), 2)
            cell_finished.set()
            await handle._notice_task
            self.assertTrue(notice.is_set())
            with patch.object(bash_module, "_live_cell_owner", return_value=asyncio.current_task()):
                handle.output()
            await asyncio.sleep(0)
        self.assertEqual(calls[2:], [
            ("bash.completed", {"handle_id": "host-opaque-1", "command": "background", "exitCode": 0}),
            ("bash.consumed", {"handle_id": "host-opaque-1", "command": "background"})])
        self.assertEqual([d["application/vnd.bash-activity+json"]["active"] for d in displays], [True, False])
        self.assertTrue(all("pid" not in d["application/vnd.bash-activity+json"] for d in displays))

    async def test_foreground_await_suppresses_notice(self):
        from rlm import repl
        cell_finished = asyncio.Event()
        calls = []
        async def host(kind, payload):
            calls.append(kind)
            return snapshot(False, "done", 0)
        with patch.object(rlm, "host_request", host), patch.object(repl, "is_active", return_value=True),              patch.object(repl, "emit"),              patch.object(bash_module, "_current_cell_completion_context", return_value=(cell_finished, asyncio.current_task())):
            handle = rlm.bash("foreground")
            await handle
            cell_finished.set()
            await handle._notice_task
        self.assertEqual(calls, ["bash.start", "bash.forget"])

    async def test_sync_cell_keeps_start_capability_until_admission(self):
        from rlm import repl
        frames, ids = [], []
        admitted = asyncio.Event()
        async def host(kind, payload):
            ids.append((kind, repl.current_host_request_cell_id()))
            if kind == "bash.start":
                await asyncio.sleep(0)
                self.assertFalse(any(frame["event"] == "done" for frame in frames))
                admitted.set()
            return snapshot(False, "done", 0)
        ns = {"bash": rlm.bash}
        with patch.object(rlm, "host_request", host), patch.object(repl, "_loop", asyncio.get_running_loop()),              patch.object(repl, "_send", side_effect=frames.append), patch.object(repl, "_drain_output"),              patch.object(repl, "emit"), patch.object(repl, "is_active", return_value=True):
            await repl._handle_execute({"id": "sync-cell", "code": "h = bash('background')"}, ns)
            await ns["h"]._notice_task
        self.assertTrue(admitted.is_set())
        self.assertEqual(ids, [("bash.start", "sync-cell"), ("bash.forget", "sync-cell"), ("bash.completed", None)])
        self.assertEqual(frames, [{"event": "done", "id": "sync-cell", "status": "ok"}])

    async def test_background_start_denial_is_a_cell_error(self):
        from rlm import repl
        frames = []
        async def host(kind, payload):
            raise RuntimeError("approval denied")
        ns = {"bash": rlm.bash}
        with patch.object(rlm, "host_request", host), patch.object(repl, "_loop", asyncio.get_running_loop()),              patch.object(repl, "_send", side_effect=frames.append), patch.object(repl, "_drain_output"),              patch.object(repl, "emit"), patch.object(repl, "is_active", return_value=True),              patch("sys.stderr.write"):
            await repl._handle_execute({"id": "denied-cell", "code": "h = bash('background')"}, ns)
            await ns["h"]._notice_task
        self.assertEqual(frames[-1], {"event": "done", "id": "denied-cell", "status": "error"})
        self.assertIn("approval denied", str(frames[0]))

    async def test_poll_error_schedules_host_cleanup(self):
        killed = asyncio.Event()
        async def host(kind, payload):
            if kind == "bash.start":
                return snapshot()
            if kind == "bash.kill":
                killed.set()
                return snapshot()
            raise RuntimeError("poll failed")
        with patch.object(rlm, "host_request", host), patch.object(bash_module, "_POLL_INTERVAL", 0):
            handle = rlm.bash("background")
            with self.assertRaisesRegex(RuntimeError, "poll failed"):
                await handle
            await killed.wait()
            await handle._kill_task

    async def test_terminal_host_error_is_not_a_fabricated_exit_code(self):
        async def host(kind, payload):
            return {**snapshot(False, "partial output"), "error": "launch failed"}
        with patch.object(rlm, "host_request", host):
            handle = rlm.bash("probe")
            with self.assertRaisesRegex(RuntimeError, "launch failed\npartial output"):
                await handle
            self.assertEqual((handle.output(), handle.tail()), ("partial output", "partial output"))

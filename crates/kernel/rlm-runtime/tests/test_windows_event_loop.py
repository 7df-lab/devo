import asyncio
import subprocess
import sys
import threading
import unittest
from unittest.mock import patch


@unittest.skipUnless(sys.platform == "win32", "Windows IOCP event loop")
class WindowsEventLoopTests(unittest.TestCase):
    def test_thread_wakeups_without_loopback_sockets(self):
        from rlm._windows_event_loop import PipeProactorEventLoop

        with patch("socket.socketpair", side_effect=AssertionError("network used")), patch(
            "tempfile.gettempdir", side_effect=AssertionError("writable temp directory required")
        ):
            loop = PipeProactorEventLoop()
            observed = []
            finished = loop.create_future()

            def notify():
                for value in range(10000):
                    loop.call_soon_threadsafe(observed.append, value)
                loop.call_soon_threadsafe(finished.set_result, None)

            thread = threading.Thread(target=notify)
            thread.start()
            try:
                loop.run_until_complete(asyncio.wait_for(finished, timeout=5))
                self.assertEqual(observed, list(range(10000)))
                # Proactor cancels and re-arms its self-reader between runs.
                loop.run_until_complete(asyncio.sleep(0.01))
            finally:
                thread.join(timeout=5)
                loop.close()
            self.assertFalse(thread.is_alive())

    def test_subprocess_pipes_remain_supported(self):
        from rlm._windows_event_loop import PipeProactorEventLoop

        async def communicate():
            child = await asyncio.create_subprocess_exec(
                sys.executable, "-c", "print('pipe output')",
                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            )
            stdout, stderr = await child.communicate()
            return child.returncode, stdout.strip(), stderr

        with patch("socket.socketpair", side_effect=AssertionError("network used")):
            loop = PipeProactorEventLoop()
            try:
                result = loop.run_until_complete(asyncio.wait_for(communicate(), timeout=5))
                self.assertEqual(result, (0, b"pipe output", b""))
            finally:
                loop.close()


if __name__ == "__main__":
    unittest.main()

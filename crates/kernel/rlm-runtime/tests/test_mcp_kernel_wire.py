"""Real REPL process/wire simulation with synthetic host replies, not live OAuth."""

import asyncio
import json
import os
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
import time
import unittest


class McpKernelWireTests(unittest.IsolatedAsyncioTestCase):
    async def test_real_kernel_discovery_call_denial_and_auth_rotation(self):
        runtime = Path(__file__).resolve().parents[1]
        bundled = runtime.parents[1] / "skills" / "assets" / "bundled"
        with TemporaryDirectory() as home:
            auth_file = Path(home) / "auth.json"

            def save_grant(expires):
                auth_file.write_text(json.dumps({"mcp:linear": {
                    "type": "oauth", "endpoint": "https://mcp.linear.app/mcp",
                    "access": "wire-synthetic-token", "refresh": "wire-synthetic-refresh",
                    "expires": expires,
                }}))

            save_grant(0)
            env = {**os.environ, "DEVO_CODING_AGENT_DIR": home,
                   "PYTHONPATH": os.pathsep.join((str(runtime / "src"), str(bundled / "linear" / "src")))}
            process = await asyncio.create_subprocess_exec(
                sys.executable, "-m", "rlm.repl", env=env,
                stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
            )

            async def read():
                line = await asyncio.wait_for(process.stdout.readline(), 10)
                self.assertTrue(line, "kernel closed unexpectedly")
                return json.loads(line)

            async def send(frame):
                process.stdin.write((json.dumps(frame) + "\n").encode())
                await process.stdin.drain()

            try:
                self.assertEqual((await read())["event"], "ready")
                await send({"type": "execute", "id": "mcp-wire", "code": """
import linear
from rlm import mcp
# The synthetic refresh host updates auth.json; the integration re-reads it.
assert await linear.linear._resolve_token()
servers = await mcp.list_servers()
assert servers == [{"server": "docs", "status": "not_started", "authStatus": "not_required", "toolCount": 0}]
assert await mcp.list_tools("docs") == []
try:
    await mcp.call_tool("docs", "create", {"name": "demo"})
except PermissionError:
    print("MCP_WIRE_OK")
else:
    raise AssertionError("denial lost")
"""})
                calls = []
                outputs = []
                while True:
                    frame = await read()
                    self.assertNotEqual(frame["event"], "error", frame)
                    if frame["event"] == "host_request":
                        self.assertEqual(frame["cell_id"], "mcp-wire")
                        request = frame["data"]
                        calls.append(request)
                        action = request["type"]
                        if action == "mcp.refresh":
                            save_grant((time.time() + 3600) * 1000)
                            result = {"status": "ok", "result": {}}
                        elif action == "mcp.list_servers":
                            result = {"status": "ok", "result": {"servers": [{
                                "server": "docs", "status": "not_started",
                                "authStatus": "not_required", "toolCount": 0,
                            }]}}
                        elif action == "mcp.list_tools":
                            result = {"status": "ok", "result": {"tools": []}}
                        elif action == "mcp.call":
                            result = {"status": "error", "error": "denied by synthetic host"}
                        else:
                            self.fail(f"unexpected host request: {action}")
                        await send({"type": "host_reply", "id": frame["id"], "data": result})
                    elif frame["event"] == "stdout":
                        outputs.append(frame["text"])
                    elif frame["event"] == "done":
                        self.assertEqual(frame, {"event": "done", "id": "mcp-wire", "status": "ok"})
                        break
                self.assertEqual(calls, [
                    {"type": "mcp.refresh", "server": "linear"},
                    {"type": "mcp.list_servers"},
                    {"type": "mcp.list_tools", "server": "docs"},
                    {"type": "mcp.call", "server": "docs", "tool": "create", "arguments": {"name": "demo"}},
                ])
                self.assertEqual("".join(outputs), "MCP_WIRE_OK\n")
            finally:
                if process.returncode is None:
                    await send({"type": "shutdown", "id": "shutdown"})
                    try:
                        await asyncio.wait_for(process.wait(), 5)
                    except TimeoutError:
                        process.kill()
                        await process.wait()


if __name__ == "__main__":
    unittest.main()

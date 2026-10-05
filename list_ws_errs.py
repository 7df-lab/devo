"""List rustc errors for the whole workspace. Never mutates source."""
import json
import subprocess
from pathlib import Path

root = Path(r"C:\Users\lenovo\Desktop\devo")
proc = subprocess.run(
    [
        "cargo",
        "check",
        "--workspace",
        "--all-targets",
        "--message-format=json",
    ],
    cwd=root,
    capture_output=True,
    text=True,
)
seen = set()
for line in proc.stdout.splitlines():
    try:
        msg = json.loads(line)
    except json.JSONDecodeError:
        continue
    if msg.get("reason") != "compiler-message":
        continue
    message = msg.get("message") or {}
    if message.get("level") != "error":
        continue
    spans = message.get("spans") or []
    prim = next((s for s in spans if s.get("is_primary")), spans[0] if spans else None)
    if not prim:
        key = message.get("message", "")[:180]
    else:
        key = f"{prim['file_name']}:{prim['line_start']}: {message.get('message','')[:160]}"
    if key in seen:
        continue
    seen.add(key)
    print(key)
print("exit", proc.returncode, "errors", len(seen))

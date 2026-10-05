"""Count Rust LOC under crates/, excluding target/."""
from pathlib import Path

root = Path(r"C:\Users\lenovo\Desktop\devo\crates")
files = []
total = 0
for path in root.rglob("*.rs"):
    if "target" in path.parts:
        continue
    try:
        text = path.read_text(encoding="utf-8")
    except UnicodeDecodeError:
        text = path.read_text(encoding="utf-8", errors="replace")
    n = text.count("\n") + (0 if text.endswith("\n") or not text else 1)
    files.append((n, path.relative_to(root).as_posix()))
    total += n
files.sort(reverse=True)
print(f"files {len(files)}")
print(f"lines {total}")
print(f"baseline 283770")
print(f"target   198639")
print(f"need_drop {max(0, total - 198639)}")
print(f"drop_pct {(283770 - total) / 283770 * 100:.2f}%")
print("--- largest ---")
for n, p in files[:40]:
    print(f"{n:6d} {p}")

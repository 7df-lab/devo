"""Select CI components without compiling unrelated Rust crates.

Unknown paths and unavailable diffs deliberately select every component.
Main-branch pushes always run the full matrix via --all.
"""
import argparse
import os
from pathlib import Path
import subprocess

COMPONENTS = ("rust", "desktop", "tui", "kernel_python")


def classify(paths):
    selected = set()
    for raw_path in paths:
        path = raw_path.replace("\\", "/")
        if path.startswith("apps/tui/"):
            selected.add("tui")
        elif path.startswith("apps/desktop/"):
            selected.add("desktop")
        elif path.startswith("crates/"):
            # Rust contracts and embedded runtime/skill assets can affect clients.
            selected.update(COMPONENTS)
        elif path.startswith(("docs/", "apps/web/")) or path in (
            "AGENTS.md", "CONTRIBUTING.md", "LICENSE",
        ) or path.startswith("README") and path.endswith(".md"):
            continue
        else:
            selected.update(COMPONENTS)
    return {component: component in selected for component in COMPONENTS}


def changed_paths(base, head):
    result = subprocess.run(
        ["git", "diff", "--no-ext-diff", "--name-only", "--no-renames", "-z", base, head, "--"],
        check=True, capture_output=True,
    )
    return [os.fsdecode(path) for path in result.stdout.split(b"\0") if path]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", default="HEAD")
    parser.add_argument("--head", default="HEAD")
    parser.add_argument("--all", action="store_true")
    parser.add_argument("--output", type=Path, help="Append GitHub Actions job outputs")
    args = parser.parse_args()
    if args.all:
        selection = dict.fromkeys(COMPONENTS, True)
    else:
        try:
            selection = classify(changed_paths(args.base, args.head))
        except (OSError, subprocess.CalledProcessError) as error:
            print(f"Diff unavailable; running full CI ({type(error).__name__}).")
            selection = dict.fromkeys(COMPONENTS, True)
    output = "".join(f"{name}={str(enabled).lower()}\n" for name, enabled in selection.items())
    print(output, end="")
    if args.output:
        with args.output.open("a", encoding="utf-8") as stream:
            stream.write(output)


if __name__ == "__main__":
    main()

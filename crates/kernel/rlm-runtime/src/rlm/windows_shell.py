"""Find Git Bash from installed locations without trusting a workspace PATH."""

from collections.abc import Iterable, Mapping
from pathlib import Path


def git_install_roots() -> Iterable[str]:
    """Read Git for Windows installer registrations in user and machine views."""
    try:
        import winreg
    except ImportError:
        return
    for hive in (winreg.HKEY_CURRENT_USER, winreg.HKEY_LOCAL_MACHINE):
        for view in (winreg.KEY_WOW64_64KEY, winreg.KEY_WOW64_32KEY):
            try:
                with winreg.OpenKey(
                    hive, r"SOFTWARE\GitForWindows", 0, winreg.KEY_READ | view
                ) as key:
                    root, kind = winreg.QueryValueEx(key, "InstallPath")
                    if kind == winreg.REG_SZ and isinstance(root, str):
                        yield root
            except OSError:
                continue


def find_windows_bash(environment: Mapping[str, str]) -> str | None:
    roots = list(git_install_roots())
    for name in ("ProgramFiles", "ProgramFiles(x86)"):
        if base := environment.get(name):
            roots.append(str(Path(base) / "Git"))
    if base := environment.get("LOCALAPPDATA"):
        roots.append(str(Path(base) / "Programs" / "Git"))
    for root in roots:
        directory = Path(root)
        if not directory.is_absolute():
            continue
        for relative in ("bin/bash.exe", "usr/bin/bash.exe"):
            candidate = directory / relative
            if candidate.is_file():
                return str(candidate)
    return None

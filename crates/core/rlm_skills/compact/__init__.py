"""Devo RLM compact skill: context compaction from the kernel.

Thin typed wrappers over `rlm.host_request` (`compact.status` / `compact.run`).
Compaction never runs mid-cell — the host schedules turn-end execution.
"""

from __future__ import annotations

from typing import Any

from rlm import host_request


async def status() -> dict[str, Any]:
    """Read current context usage (`tokens`, `percent`, `scheduled`, …)."""
    return await host_request("compact.status")


async def run(instructions: str | None = None) -> dict[str, Any]:
    """Schedule context compaction for the end of the current turn."""
    if instructions is not None and not isinstance(instructions, str):
        raise TypeError(f"instructions must be str or None, got {type(instructions).__name__}")
    payload: dict[str, Any] = {}
    if instructions is not None:
        payload["instructions"] = instructions
    return await host_request("compact.run", payload)

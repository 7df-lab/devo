"""Devo RLM refine skill: continual harness refinement from the kernel.

Thin typed wrappers over `rlm.host_request` (`refine.status` / `refine.run`).
Refinement never runs mid-cell — the host schedules turn-boundary apply.
"""

from __future__ import annotations

from typing import Any

from rlm import host_request


async def status() -> dict[str, Any]:
    """Read current refine state (`pending`, `inFlight`)."""
    return await host_request("refine.status")


async def run(
    instructions: str | None = None,
    global_: bool = False,
) -> dict[str, Any]:
    """Schedule continual harness refinement for the end of the current turn."""
    if instructions is not None and not isinstance(instructions, str):
        raise TypeError(
            f"instructions must be str or None, got {type(instructions).__name__}"
        )
    if not isinstance(global_, bool):
        raise TypeError(f"global_ must be bool, got {type(global_).__name__}")
    payload: dict[str, Any] = {}
    if instructions is not None:
        payload["instructions"] = instructions
    if global_:
        payload["global"] = True
    return await host_request("refine.run", payload)

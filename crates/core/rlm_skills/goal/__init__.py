"""Devo RLM goal skill: thread goal control from the kernel.

Thin typed wrappers over `rlm.host_request` (`goal.get` / `goal.create` /
`goal.complete`). Host maps these to Native `session/goal/*` (create/complete
still pending full bridge; get returns structured null until wired).
"""

from __future__ import annotations

from typing import Any

from rlm import host_request


async def get() -> dict[str, Any]:
    """Read the current thread goal."""
    return await host_request("goal.get")


async def create(objective: str, token_budget: int | None = None) -> dict[str, Any]:
    """Start a new active thread goal."""
    if not isinstance(objective, str):
        raise TypeError(f"objective must be str, got {type(objective).__name__}")
    if token_budget is not None and not isinstance(token_budget, int):
        raise TypeError(f"token_budget must be int or None, got {type(token_budget).__name__}")
    payload: dict[str, Any] = {"objective": objective}
    if token_budget is not None:
        payload["token_budget"] = token_budget
    return await host_request("goal.create", payload)


async def complete() -> dict[str, Any]:
    """Mark the existing thread goal achieved."""
    return await host_request("goal.complete")

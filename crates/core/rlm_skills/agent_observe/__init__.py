"""Devo RLM agent_observe skill: nuclear-family roster from the kernel."""

from __future__ import annotations

from typing import Any

from rlm import host_request


async def list_agents() -> dict[str, Any]:
    """List the full nuclear family: parent, siblings, children, active or not."""
    return await host_request("agent_observe.list")


async def get_agent(target: str) -> dict[str, Any]:
    """Read one live session summary by active id, session id/name, or suffix."""
    if not isinstance(target, str):
        raise TypeError(f"target must be str, got {type(target).__name__}")
    return await host_request("agent_observe.get", {"target": target})


async def recent_messages(
    target: str,
    limit: int = 8,
    max_chars: int = 800,
) -> dict[str, Any]:
    """Read bounded recent message previews from an active session."""
    if not isinstance(target, str):
        raise TypeError(f"target must be str, got {type(target).__name__}")
    if not isinstance(limit, int):
        raise TypeError(f"limit must be int, got {type(limit).__name__}")
    if not isinstance(max_chars, int):
        raise TypeError(f"max_chars must be int, got {type(max_chars).__name__}")
    return await host_request(
        "agent_observe.recent",
        {
            "target": target,
            "limit": limit,
            "max_chars": max_chars,
        },
    )


# Short aliases kept for earlier Devo stubs / prompts.
list = list_agents
get = get_agent
recent = recent_messages

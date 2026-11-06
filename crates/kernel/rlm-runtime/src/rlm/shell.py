"""Shell execution through the host approval pipeline."""

from __future__ import annotations
from typing import Any


async def shell(
    command: str,
    *,
    sandbox_permissions: str = "use_default",
    justification: str | None = None,
    additional_permissions: dict[str, Any] | None = None,
    prefix_rule: list[str] | None = None,
    timeout_ms: int | None = None,
    workdir: str | None = None,
) -> dict[str, Any]:
    """Run a host shell command after authorization, returning its structured result.

    Use with_additional_permissions for narrow grants, or require_escalated for
    an approved unsandboxed command. A denial raises RuntimeError. For a live
    handle with background execution, use bash(), which also runs on the host.
    workdir is resolved by the host relative to the session workspace; omission
    uses that workspace, not the Python process's current directory.
    """
    if not isinstance(command, str) or not command.strip():
        raise ValueError("command must be a non-empty string")
    from . import host_request

    payload: dict[str, Any] = {
        "command": command,
        "sandbox_permissions": sandbox_permissions,
    }
    for key, value in {
        "justification": justification,
        "additional_permissions": additional_permissions,
        "prefix_rule": prefix_rule,
        "timeout_ms": timeout_ms,
        "workdir": workdir,
    }.items():
        if value is not None:
            payload[key] = value
    return await host_request("bash", payload)

"""Generic MCP tools exposed through the session's Rust host.

The host owns connections, credentials, discovery, and tool-call approval. This
module never starts a second transport when a host request fails.
"""

from __future__ import annotations

from typing import Any

__all__ = ["call_tool", "list_tools"]


async def _request(action: str, payload: dict[str, Any]) -> Any:
    from . import repl

    # Transport errors and cancellation propagate without retrying the tool.
    reply = await repl.host_request({**payload, "type": action})
    if not isinstance(reply, dict):
        raise RuntimeError(f"{action} host response must be an object")
    if reply.get("status") != "ok":
        error = reply.get("error")
        normalized = " ".join(error.lower().split()) if isinstance(error, str) else ""
        if normalized in {
            f"{kind} {label}: {action}"
            for kind in ("unknown", "unsupported")
            for label in ("host_request action", "host request action", "action")
        }:
            raise RuntimeError(
                f"The Devo host does not support {action}; upgrade Devo and restart "
                "the session. Generic MCP tools require the Rust host."
            )
        message = str(error or f"{action} host request failed")
        # Preserve the existing call_tool host-rejection contract. The host's
        # error envelope does not distinguish denial from other call failures.
        if action == "mcp.call":
            raise PermissionError(message)
        raise RuntimeError(message)
    return reply.get("result")


async def list_tools(server: str) -> list[dict[str, Any]]:
    """List one server's tools through the Rust host.

    Return ``[{name, description, inputSchema}]``. Missing or unsupported hosts
    and discovery errors are surfaced to the caller, with no local fallback.
    """
    _validate_name(server, "server")
    result = await _request("mcp.list_tools", {"server": server})
    if not isinstance(result, dict) or not isinstance(result.get("tools"), list):
        raise RuntimeError("mcp.list_tools host response must contain result.tools")

    tools: list[dict[str, Any]] = []
    for tool in result["tools"]:
        if not isinstance(tool, dict) or tool.get("server") != server:
            continue
        name = tool.get("name")
        if not isinstance(name, str):
            continue
        description = tool.get("description")
        schema = tool.get("inputSchema")
        tools.append(
            {
                "name": name,
                "description": (
                    description if isinstance(description, str) else str(description or "")
                ),
                "inputSchema": schema if isinstance(schema, dict) else {},
            }
        )
    return tools


async def call_tool(
    server: str, tool: str, arguments: dict[str, Any] | None = None
) -> dict[str, Any]:
    """Call a tool through the Rust host's approval pipeline.

    Return the complete MCP CallToolResult unchanged, including ``content``,
    ``structuredContent``, and ``isError``. A tool-level ``isError`` result is
    data, not a host rejection. Host rejections raise PermissionError; transport
    failures and unsupported hosts raise errors without executing locally.
    """
    _validate_name(server, "server")
    _validate_name(tool, "tool")
    if arguments is not None and not isinstance(arguments, dict):
        raise TypeError("arguments must be a dict or None")
    result = await _request(
        "mcp.call", {"server": server, "tool": tool, "arguments": arguments or {}}
    )
    if not isinstance(result, dict):
        raise RuntimeError("mcp.call host response must contain a result object")
    return result


def _validate_name(value: str, label: str) -> None:
    if not isinstance(value, str) or not value:
        raise TypeError(f"{label} must be a non-empty string")

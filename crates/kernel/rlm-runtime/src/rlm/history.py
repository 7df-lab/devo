"""Read recorded messages and tool evidence from this session's active branch."""
from typing import Any


async def search(
    query: str = "", *, kind: str | None = None, call_id: str | None = None,
    before: str | None = None, limit: int = 6,
) -> dict[str, Any]:
    """Find literal matches, newest first. Pass next_before to page backwards."""
    from . import host_request

    return await host_request("rlm.history.search", {
        "query": query, "kind": kind, "call_id": call_id, "before": before, "limit": limit,
    })


async def read(item_id: str, *, offset: int = 0, max_chars: int = 3000,
               around: int = 0) -> dict[str, Any]:
    """Read an item or call:<id>. Offsets count Unicode characters; around adds previews."""
    from . import host_request

    return await host_request("rlm.history.read", {
        "item_id": item_id, "offset": offset, "max_chars": max_chars, "around": around,
    })

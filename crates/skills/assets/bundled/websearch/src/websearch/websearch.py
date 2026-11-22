"""Text compatibility wrapper around Devo's selected web search."""

from __future__ import annotations

import json


async def run(
    query: str,
    *,
    max_output: int = 8192,
    timeout: int | None = None,
    num_results: int | None = None,
) -> str:
    """Search through the host once, using the current turn's selected backend.

    ``timeout`` and ``num_results`` are accepted for compatibility but no longer
    override backend settings. Host errors propagate without trying another route.
    """
    from rlm import web_search

    result = await web_search(query)
    if isinstance(result, str):
        text = result
    elif isinstance(result, dict) and isinstance(result.get("stdout"), str):
        text = result["stdout"]
    elif isinstance(result, dict) and isinstance(result.get("content"), list):
        blocks = result["content"]
        text = "\n\n".join(
            block["text"]
            for block in blocks
            if isinstance(block, dict)
            and block.get("type") == "text"
            and isinstance(block.get("text"), str)
            and block["text"].strip()
        )
        if not text:
            text = json.dumps(result, ensure_ascii=False) if blocks or result.get("structuredContent") else ""
    else:
        text = json.dumps(result, ensure_ascii=False)
    if not text.strip():
        return "No text results returned by the selected web search backend."[:max(0, max_output)]

    limit = max(0, max_output)
    if len(text) <= limit:
        return text
    marker = f"\n... [output truncated, {len(text)} chars total] ...\n"
    if limit <= len(marker):
        return text[:limit]
    left = (limit - len(marker)) // 2
    right = limit - len(marker) - left
    return text[:left] + marker + text[-right:]

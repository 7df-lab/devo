"""Websearch skill: prefer Devo-hosted search, then Serper fallback."""

from __future__ import annotations

import json
import os
from pathlib import Path

import httpx


def _env_int(name: str, default: int) -> int:
    """Read an int from the environment, falling back to default on bad values."""
    try:
        return int(os.environ[name])
    except (KeyError, ValueError):
        return default


def _agent_dir() -> Path:
    """Resolve the agent config dir the same way the runtime does."""
    raw = (
        os.environ.get("DEVO_CODING_AGENT_DIR")
        or os.environ.get("DEVO_HOME")
        or os.environ.get("PI_CODING_AGENT_DIR")
        or str(Path.home() / ".devo")
    )
    return Path(raw).expanduser()


def _resolve_api_key() -> str:
    # Read auth.json on each call (not just the injected env var) so a key added
    # via /login after the kernel started is still picked up. Env var wins.
    env_key = os.environ.get("SERPER_API_KEY", "").strip()
    if env_key:
        return env_key

    try:
        auth = json.loads((_agent_dir() / "auth.json").read_text())
        cred = auth.get("serper") if isinstance(auth, dict) else None
        if isinstance(cred, dict) and cred.get("type") == "api_key":
            return _resolve_config_value(str(cred.get("key") or ""))
    except (OSError, ValueError):
        pass
    return ""


def _resolve_config_value(value: str) -> str:
    # Stored keys may be a literal or an env-var name; "!command" refs can't be run
    # safely here, so skip them (the agent injects those resolved at build time).
    value = value.strip()
    if not value or value.startswith("!"):
        return ""
    return (os.environ.get(value) or value).strip()


def _format_serper_results(data: dict, query: str, num_results: int = 5) -> str:
    """Format a Serper API response into readable text."""
    sections: list[str] = []

    kg = data.get("knowledgeGraph")
    if kg:
        kg_lines: list[str] = []
        title = (kg.get("title") or "").strip()
        if title:
            kg_lines.append(f"Knowledge Graph: {title}")
        description = (kg.get("description") or "").strip()
        if description:
            kg_lines.append(description)
        for key, value in (kg.get("attributes") or {}).items():
            text = str(value).strip()
            if text:
                kg_lines.append(f"{key}: {text}")
        if kg_lines:
            sections.append("\n".join(kg_lines))

    for i, result in enumerate((data.get("organic") or [])[:num_results]):
        title = (result.get("title") or "").strip() or "Untitled"
        lines = [f"Result {i}: {title}"]
        link = (result.get("link") or "").strip()
        if link:
            lines.append(f"URL: {link}")
        snippet = (result.get("snippet") or "").strip()
        if snippet:
            lines.append(snippet)
        sections.append("\n".join(lines))

    people_also_ask = data.get("peopleAlsoAsk") or []
    if people_also_ask:
        max_q = max(1, min(3, len(people_also_ask)))
        questions: list[str] = []
        for item in people_also_ask[:max_q]:
            question = (item.get("question") or "").strip()
            if not question:
                continue
            entry = f"Q: {question}"
            answer = (item.get("snippet") or "").strip()
            if answer:
                entry += f"\nA: {answer}"
            questions.append(entry)
        if questions:
            sections.append("People Also Ask:\n" + "\n".join(questions))

    if not sections:
        return f"No results returned for query: {query}"

    return "\n\n---\n\n".join(sections)


async def _fetch_serper(query: str, api_key: str, timeout: int = 45, num_results: int = 5) -> str:
    """Execute a single Serper API search."""
    try:
        async with httpx.AsyncClient(timeout=timeout) as client:
            resp = await client.post(
                "https://google.serper.dev/search",
                json={"q": query},
                headers={
                    "X-API-KEY": api_key,
                    "Content-Type": "application/json",
                },
            )
            resp.raise_for_status()
            data = resp.json()
    except httpx.HTTPStatusError as e:
        body = e.response.text if e.response is not None else ""
        raise RuntimeError(f"Serper search error ({e.response.status_code}): {body}") from e

    return _format_serper_results(data, query, num_results=num_results)


def _truncate(output: str, max_output: int) -> str:
    if len(output) <= max_output:
        return output
    total = len(output)
    marker = f"\n... [output truncated, {total} chars total] ...\n"
    half = max(0, (max_output - len(marker)) // 2)
    output = output[:half] + marker + output[len(output) - half :]
    if len(output) > max_output:
        output = output[:max_output]
    return output


def _setup_reminder() -> str:
    """Guide the user when no hosted search and no Serper key are available."""
    return (
        "Web search is not available yet.\n"
        "Prefer Devo's built-in search first (do not jump straight to Serper):\n"
        "  1. Provider-hosted `web_search` — set `[tools.web_search] mode = \"provider\"` "
        "and use a model on Anthropic Messages or OpenAI Responses that supports hosted search. "
        "When that tool appears in your tool list, call it instead of this skill.\n"
        "  2. Local `web_search` — set `[tools.web_search] mode = \"local\"` with a provider under "
        "`[tools.web_search.local_providers.<id>]` using `kind = \"exa\"` or `kind = \"tavily\"` "
        "(API keys via `/login` / auth.json credentials).\n"
        "Third-party fallback (this skill / Serper):\n"
        "  1. Get a free API key at https://serper.dev\n"
        "  2. In Devo, run /login → MCP Connections → \"Serper (web search)\" and paste the key.\n"
        "Do not ask the user to set environment variables for Serper."
    )


async def _try_hosted_web_search(query: str, num_results: int | None) -> str | None:
    """Use Devo local/host `web.search` (Exa/Tavily) when configured for this turn.

    Returns formatted text on success, or None when hosted search is unavailable
    so the caller can fall back to Serper. Other errors propagate.
    """
    from rlm import host_request

    payload: dict = {"query": query}
    if num_results is not None:
        payload["max_results"] = num_results
    try:
        result = await host_request("web.search", payload)
    except RuntimeError as error:
        message = str(error).lower()
        if (
            "not configured" in message
            or "needsconfiguration" in message
            or "needs configuration" in message
        ):
            return None
        raise

    if isinstance(result, str) and result.strip():
        return result
    if isinstance(result, dict):
        for key in ("stdout", "text", "content", "result"):
            value = result.get(key)
            if isinstance(value, str) and value.strip():
                return value
        summary = result.get("summary")
        if isinstance(summary, str) and summary.strip():
            return summary
    return None


async def run(
    query: str,
    *,
    max_output: int = 8192,
    timeout: int | None = None,
    num_results: int | None = None,
) -> str:
    """Search the web, preferring Devo-hosted search over Serper.

    Order:
      1. Host `web.search` (local Exa / Tavily when `tools.web_search` mode is local).
      2. Serper third-party API when a key is configured.
      3. Setup reminder listing provider-hosted `web_search`, Exa/Tavily, and Serper.

    If the model already has a `web_search` tool in its tool list (provider-hosted
    or local), prefer calling that tool instead of this skill.
    """
    if timeout is None:
        timeout = _env_int("DEVO_WEBSEARCH_TIMEOUT", 45)
    if num_results is None:
        num_results = _env_int("DEVO_WEBSEARCH_NUM_RESULTS", 5)

    hosted = await _try_hosted_web_search(query, num_results)
    if hosted is not None:
        output = f'Results for query "{query}" (Devo web_search):\n\n{hosted}'
        return _truncate(output, max_output)

    api_key = _resolve_api_key()
    if not api_key:
        return _setup_reminder()

    try:
        result = await _fetch_serper(query, api_key, timeout=timeout, num_results=num_results)
    except Exception as e:
        result = f"Error searching for '{query}': {e}"
    output = f'Results for query "{query}" (Serper):\n\n{result}'
    return _truncate(output, max_output)

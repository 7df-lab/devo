"""Base class for MCP-client integrations exposed in the RLM kernel.

An integration is a Python skill package that subclasses :class:`McpIntegration`,
declares the MCP ``server`` it targets, and is imported in the kernel like any
other skill. Tools are auto-discovered from the server and bound as async
methods, so the agent writes ordinary Python:

    import linear
    issues = await linear.list_issues(team="Engineering")

Authored integration credentials live in their login host's ``auth.json``
(survives kernel rebuilds). This module reads that file directly for the common case; on token
expiry it asks the host to refresh via ``rlm.host_request("mcp.refresh", ...)``
and re-reads. Interactive login runs host-side, never here. The current Rust
host does not implement this legacy auth.json refresh contract and reports that
explicitly; a native configured MCP server is instead used through ``rlm.mcp``.
"""

from __future__ import annotations

import asyncio
import json
import math
import os
import time
from contextlib import AsyncExitStack
from pathlib import Path
from typing import Any

from . import host_request

__all__ = ["McpIntegration", "McpToolError", "NotEnabled"]

# Stored access tokens are treated as expired this many seconds early so a token
# never dies mid-request. Mirrors the host's refresh buffer.
_EXPIRY_SKEW_SECONDS = 30


class NotEnabled(RuntimeError):
    """Raised when an integration has no usable credentials.

    The integration is installed but not logged in. The message tells the agent
    how to enable it so it can relay that to the user rather than retrying.
    """

    def __init__(self, server: str):
        self.server = server
        super().__init__(
            f"The '{server}' integration is not enabled: no credentials found. "
            f"Connect through its login host (`/mcp login {server}` or `/login` → Services "
            "where supported). Native Rust-configured MCP servers use rlm.mcp instead. "
            "Do not ask the user to set environment variables."
        )


class McpToolError(RuntimeError):
    """Raised when an MCP tool call returns a result flagged as an error."""


def _agent_dir() -> Path:
    """Resolve the Devo config dir the same way the host does."""
    raw = (
        os.environ.get("DEVO_CODING_AGENT_DIR")
        or os.environ.get("DEVO_HOME")
        or os.environ.get("PI_CODING_AGENT_DIR")
        or str(Path.home() / ".devo")
    )
    # resolve() so a relative env override reads auth.json from the right place,
    # not relative to the kernel's cwd.
    return Path(raw).expanduser().resolve()


def _read_auth(provider: str) -> dict[str, Any] | None:
    """Read one credential entry from auth.json. Returns None if absent/unreadable."""
    try:
        data = json.loads((_agent_dir() / "auth.json").read_text())
    except (OSError, ValueError):
        return None
    if not isinstance(data, dict):
        return None
    cred = data.get(provider)
    return cred if isinstance(cred, dict) else None


def _resolve_config_value(value: str) -> str:
    """Resolve a stored api_key value the way the host does.

    A value may be a literal, an env-var name, or a `!command` indirection. The
    command form can't run safely in the kernel (the host injects those resolved),
    so skip it; otherwise treat the value as an env-var name if set, else literal.
    """
    value = value.strip()
    if not value or value.startswith("!"):
        return ""
    return (os.environ.get(value) or value).strip()


def _resolve_streamable_http():
    """Return an SDK streamable-HTTP transport callable.

    SDK versions vary: some expose ``streamablehttp_client(url, headers=...)``,
    others ``streamable_http_client(url, *, http_client=...)``, and some expose
    both with *different* signatures. Imported lazily so importing an integration
    package never hard-fails when ``mcp`` is absent.
    """
    try:
        from mcp.client import streamable_http as mod  # noqa: PLC0415
    except ImportError as error:
        raise RuntimeError(
            "The MCP Python SDK (`mcp`) is not installed in this kernel's "
            "Python environment, so Linear/Notion-style integrations cannot "
            "connect. Install it with: python -m pip install 'mcp>=2,<3'"
        ) from error

    for name in ("streamablehttp_client", "streamable_http_client"):
        fn = getattr(mod, name, None)
        if fn is not None:
            return fn
    raise ImportError(
        "the installed `mcp` SDK exposes no streamable-HTTP client; upgrade `mcp`"
    )


class McpIntegration:
    """Subclass and set :attr:`server` (and :attr:`url` for remote servers).

    Tools are discovered on first use and bound as async methods via
    ``__getattr__``; ``await self.call_tool(name, args)`` is the explicit escape
    hatch and the hook for hand-written typed wrappers.
    """

    #: Credential / config key for this integration (matches the auth.json entry
    #: ``mcp:<server>`` and the mcpServers settings key).
    server: str = ""

    #: Remote MCP endpoint. Required unless a subclass overrides ``_open_streams``.
    url: str | None = None

    #: Optional env var holding a static bearer token (used instead of auth.json OAuth).
    bearer_token_env: str | None = None

    def __init__(self) -> None:
        if not self.server:
            raise ValueError(f"{type(self).__name__} must set a non-empty `server`")
        self._tools: dict[str, Any] | None = None
        self._lock = asyncio.Lock()
        self._refresh_lock = asyncio.Lock()

    # -- credentials --------------------------------------------------------

    @property
    def _provider_id(self) -> str:
        return f"mcp:{self.server}"

    def _token(self) -> str | None:
        """Current usable bearer token, or None if missing/expired (needs refresh).

        A static bearer-token env var wins (matches the host's `isAuthed` check);
        otherwise read auth.json. OAuth tokens are only returned while still fresh.
        """
        if self.bearer_token_env:
            env_token = os.environ.get(self.bearer_token_env, "").strip()
            if env_token:
                return env_token
        cred = _read_auth(self._provider_id)
        if cred is None:
            return None
        # Never refresh or transmit a credential explicitly bound elsewhere.
        # Legacy built-in grants may lack a binding; their endpoint is authored
        # in code and cannot be replaced by same-named user MCP configuration.
        endpoint = cred.get("endpoint")
        if endpoint is not None and endpoint != self.url:
            raise RuntimeError(
                f"Stored credentials for '{self.server}' belong to another endpoint. "
                "Reconnect the integration; no credential was sent."
            )
        if cred.get("type") == "api_key":
            key = cred.get("key")
            return (_resolve_config_value(key) or None) if isinstance(key, str) else None
        if cred.get("type") != "oauth":
            return None
        # OAuth credential: {access, refresh, expires(ms)}. Malformed expiry
        # (including booleans and infinity) must not make a token immortal.
        access = cred.get("access")
        expires = cred.get("expires")
        fresh = (
            type(expires) in (int, float)
            and 0 <= expires <= 2**53 - 1
            and math.isfinite(expires)
            and time.time() * 1000 < expires - _EXPIRY_SKEW_SECONDS * 1000
        )
        if isinstance(access, str) and access.strip() and fresh:
            return access
        return None  # signal: needs refresh

    async def _resolve_token(self) -> str:
        token = self._token()
        if token:
            return token
        # Concurrent calls share one refresh and re-read the store afterwards.
        # This matters for providers that rotate single-use refresh tokens.
        async with self._refresh_lock:
            token = self._token()
            if token:
                return token
            if _read_auth(self._provider_id) is None:
                raise NotEnabled(self.server)
            refresh_failure: str | None = None
            try:
                await host_request("mcp.refresh", {"server": self.server})
            except RuntimeError as error:
                # Provider errors may contain response bodies or credentials.
                # Keep the recovery actionable without leaking them in a
                # chained traceback. Cancellation is deliberately not caught.
                reason = (
                    "The Rust host does not refresh authored auth.json credentials; "
                    "native configured servers use rlm.mcp instead. "
                    if str(error).startswith(
                        "This Rust host cannot refresh authored integration credentials in auth.json."
                    ) else ""
                )
                refresh_failure = (
                    f"Failed to refresh credentials for '{self.server}'. {reason}"
                    "Reconnect using the host that owns this integration's login."
                )
            # A second host may have reconnected while this request failed.
            # Re-read outside the exception handler: the new grant must pass
            # the same binding/expiry checks, without chaining a provider body.
            token = self._token()
            if token:
                return token
            if refresh_failure is not None:
                raise RuntimeError(refresh_failure) from None
            # Acknowledgement is not proof of rotation. Do not turn a stale or
            # malformed grant into the misleading 'not enabled' diagnosis.
            raise RuntimeError(
                f"Credential refresh for '{self.server}' returned without usable credentials. "
                "Reconnect the integration using the host that owns its login."
            )

    # -- connection ---------------------------------------------------------

    async def _resolve_config(self) -> tuple[str | None, dict[str, str]]:
        """The integration's own (url, extra_headers). Never consults user
        ``mcpServers`` entries: a same-named entry must not repoint an authored
        integration, whose credentials (stored or env-sourced) would follow.
        """
        return self.url, {}

    async def _open_session(self, stack: AsyncExitStack):
        """Open an initialized MCP ClientSession bound to ``stack``.

        Override for non-HTTP transports (e.g. stdio). The default connects over
        streamable HTTP to the class ``url`` with a Bearer token from auth.json.
        """
        import inspect  # noqa: PLC0415

        try:
            from mcp import ClientSession  # noqa: PLC0415
        except ImportError as error:
            raise RuntimeError(
                "The MCP Python SDK (`mcp`) is not installed in this kernel's "
                "Python environment, so Linear/Notion-style integrations cannot "
                "connect. Install it with: python -m pip install 'mcp>=2,<3'"
            ) from error

        url, extra_headers = await self._resolve_config()
        if not url:
            raise ValueError(
                f"{type(self).__name__} must set `url` or override `_open_session`"
            )
        token = await self._resolve_token()
        transport = _resolve_streamable_http()
        # Extra configured headers first, Authorization last so it always wins.
        auth_header = {**extra_headers, "Authorization": f"Bearer {token}"}

        # SDK signatures vary: some take headers=, others only http_client=.
        params = inspect.signature(transport).parameters
        if "headers" in params:
            cm = transport(url, headers=auth_header)
        elif "http_client" in params:
            # This SDK shape requires its companion httpx2 client (the transport calls client.sse()).
            import httpx2  # noqa: PLC0415

            # SDK-factory timeouts; a default client's 5s read cap drops idle SSE streams.
            # No redirects: a redirecting endpoint must not receive the bearer header.
            client = await stack.enter_async_context(
                httpx2.AsyncClient(
                    headers=auth_header,
                    timeout=httpx2.Timeout(30.0, read=300.0),
                    follow_redirects=False,
                )
            )
            cm = transport(url, http_client=client)
        else:
            raise RuntimeError(
                f"unsupported mcp streamable-HTTP client signature: {tuple(params)}"
            )

        read, write, *_ = await stack.enter_async_context(cm)
        session = await stack.enter_async_context(ClientSession(read, write))
        await session.initialize()
        return session

    # -- tools --------------------------------------------------------------

    async def list_tools(self) -> list[dict[str, Any]]:
        """Verify the connection and return its current tools and schemas.

        An explicit listing reconnects even when tools are cached. A failed
        verification clears stale discovery, and never retries a tool call.
        """
        async with self._lock:
            await self._discover_tools()
            return [dict(t) for t in (self._tools or {}).values()]

    async def _ensure_tools(self) -> None:
        if self._tools is not None:
            return
        async with self._lock:
            if self._tools is None:
                await self._discover_tools()

    async def _discover_tools(self) -> None:
        # Caller holds _lock. Clear first so a failed reconnect never leaves a
        # stale schema presented as a current, usable connection.
        self._tools = None
        async with AsyncExitStack() as stack:
            session = await self._open_session(stack)
            resp = await session.list_tools()
            tools: dict[str, Any] = {}
            for t in resp.tools:
                # mcp>=2 exposes input_schema; inputSchema is the wire alias.
                schema = getattr(t, "input_schema", None)
                if schema is None:
                    schema = getattr(t, "inputSchema", None)
                tools[t.name] = {
                    "name": t.name,
                    "description": getattr(t, "description", "") or "",
                    "inputSchema": schema if isinstance(schema, dict) else {},
                }
            self._tools = tools

    async def call_tool(self, tool: str, arguments: dict[str, Any] | None = None) -> Any:
        """Call ``tool`` on the server and return its parsed result.

        Opens a fresh session per call: MCP sessions are not safe to hold across
        the kernel's snapshot/restore, and per-call connect keeps this robust to
        idle sessions and token rotation at modest latency cost.
        """
        async with AsyncExitStack() as stack:
            session = await self._open_session(stack)
            result = await session.call_tool(tool, arguments or {})
        return _parse_result(result)

    def __getattr__(self, name: str):
        # Only reached for names not found normally; bind as an async tool call.
        if name.startswith("_"):
            raise AttributeError(name)

        async def _call(**kwargs: Any) -> Any:
            await self._ensure_tools()
            if self._tools is not None and name not in self._tools:
                available = ", ".join(sorted(self._tools)) or "(none)"
                raise AttributeError(
                    f"'{self.server}' has no tool '{name}'. Available: {available}"
                )
            return await self.call_tool(name, kwargs)

        _call.__name__ = name
        _call.__qualname__ = f"{type(self).__name__}.{name}"
        if self._tools and name in self._tools:
            schema = self._tools[name].get("inputSchema") or {}
            desc = self._tools[name].get("description") or ""
            _call.__doc__ = f"{desc}\n\nArguments (JSON Schema):\n{json.dumps(schema, indent=2)}"
        return _call


def _parse_result(result: Any) -> Any:
    """Normalize a CallToolResult into plain Python (structured output preferred).

    Raises McpToolError when the server flags the result as an error, so a failed
    tool call doesn't look like a successful one to the caller.
    """
    texts: list[str] = []
    for block in getattr(result, "content", None) or []:
        text = getattr(block, "text", None)
        if text is not None:
            texts.append(text)
    is_error = getattr(result, "is_error", getattr(result, "isError", False))
    if is_error:
        raise McpToolError("\n".join(texts) or "MCP tool returned an error")

    structured = getattr(result, "structured_content", getattr(result, "structuredContent", None))
    if structured is not None:  # falsy-but-valid payloads ({} / []) are real results
        return structured
    if texts:
        return "\n".join(texts)

    # Non-text content (images, embedded resources): return them as plain dicts
    # rather than the opaque SDK object so callers get usable data.
    blocks = getattr(result, "content", None) or []
    if blocks:
        return [b.model_dump(mode="json") if hasattr(b, "model_dump") else b for b in blocks]
    return result

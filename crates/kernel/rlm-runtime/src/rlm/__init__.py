"""Tiny rlm-compatible kernel shim for Devo."""

from __future__ import annotations

import sys
import types
from dataclasses import dataclass
from pathlib import Path
import os
from typing import Any

from .bash import BashHandle, BashResult, bash
from .harness import HarnessEntry, HarnessScope, HarnessState, RefinementEvent, get_harness_state

_NOT_CALLABLE_MESSAGE = "'rlm' is not callable; spawn a child with: handle = await rlm.spawn('sub-task', name='worker')"
_RENAMED_RUN_MESSAGE = "rlm.run was renamed; spawn a child with: handle = await rlm.spawn('sub-task', name='worker')"


@dataclass(frozen=True)
class RLMSpawnHandle:
    rlm_child_id: str
    name: str
    session_dir: Path
    model: str


@dataclass(frozen=True)
class RLMCreateSessionHandle:
    active_session_id: str
    session_id: str
    name: str
    session_file: Path
    model: str


@dataclass(frozen=True)
class RLMModel:
    provider: str
    id: str
    name: str
    selector: str


@dataclass(frozen=True)
class RLMSubagent:
    rlm_child_id: str
    active_session_id: str | None
    session_id: str | None
    session_name: str
    session_dir: Path
    status: str


def _spawn_handle_from_payload(payload: Any) -> RLMSpawnHandle:
    if not isinstance(payload, dict):
        raise RuntimeError("rlm.spawn returned an invalid spawn handle")
    child_id = payload.get("rlm_child_id")
    name = payload.get("name")
    session_dir = payload.get("session_dir")
    model = payload.get("model")
    if not all(isinstance(value, str) and value for value in (child_id, name, session_dir, model)):
        raise RuntimeError("rlm.spawn returned an invalid spawn handle")
    return RLMSpawnHandle(
        rlm_child_id=child_id,
        name=name,
        session_dir=Path(session_dir),
        model=model,
    )


def _create_session_handle_from_payload(payload: Any) -> RLMCreateSessionHandle:
    if not isinstance(payload, dict):
        raise RuntimeError("rlm.create_session returned an invalid payload")
    active_session_id = payload.get("active_session_id")
    session_id = payload.get("session_id")
    name = payload.get("name")
    session_file = payload.get("session_file")
    model = payload.get("model")
    if not all(isinstance(value, str) and value for value in (active_session_id, session_id, name, session_file, model)):
        raise RuntimeError("rlm.create_session returned an invalid payload structure")
    return RLMCreateSessionHandle(
        active_session_id=active_session_id,
        session_id=session_id,
        name=name,
        session_file=Path(session_file),
        model=model,
    )


def _parse_host_reply(request_type: str, reply: dict[str, Any]) -> dict[str, Any]:
    status = reply.get("status")
    if status == "ok":
        return reply["result"]
    if status == "error":
        raise RuntimeError(str(reply.get("error") or f"host request {request_type} failed"))
    raise RuntimeError(f"host request {request_type} returned unexpected status: {status!r}")


async def host_request(request_type: str, payload: dict[str, Any] | None = None) -> dict[str, Any]:
    """Send a typed request to the Devo host and await its reply.

    This is the kernel side of the generic host bridge: Python skills call
    ``await host_request("<type>", {...})`` and the TypeScript host dispatches
    on the type. Raises RuntimeError when the host reports an error or when no
    handler for the type is registered in this session.
    """
    if not isinstance(request_type, str) or not request_type:
        raise TypeError("request_type must be a non-empty str")
    if payload is not None and not isinstance(payload, dict):
        raise TypeError(f"payload must be a dict or None, got {type(payload).__name__}")
    from . import repl

    # request_type goes last so a payload "type" key cannot reroute the request.
    reply = await repl.host_request({**(payload or {}), "type": request_type})
    return _parse_host_reply(request_type, reply)


async def web_search(query: str) -> dict[str, Any]:
    """Use the configured local web-search provider through the Rust host.

    Provider-hosted web search is attached to the outer model request instead;
    this helper is for turns configured with local search.
    """
    if not isinstance(query, str) or not query.strip():
        raise ValueError("query must be a non-empty string")
    return await host_request("web.search", {"query": query.strip()})


async def web_fetch(
    url: str,
    *,
    format: str | None = None,
    timeout: int | None = None,
) -> dict[str, Any]:
    """Fetch a URL through the Rust host, including its permission checks."""
    if not isinstance(url, str) or not url.strip():
        raise ValueError("url must be a non-empty string")
    payload: dict[str, Any] = {"url": url.strip()}
    if format is not None:
        payload["format"] = format
    if timeout is not None:
        payload["timeout"] = timeout
    return await host_request("web.fetch", payload)


async def update_plan(
    plan: list[dict[str, str]], *, explanation: str | None = None
) -> dict[str, Any]:
    """Publish a visible task plan through the Rust session host."""
    if not isinstance(plan, list):
        raise TypeError("plan must be a list of plan items")
    payload: dict[str, Any] = {"plan": plan}
    if explanation is not None:
        payload["explanation"] = explanation
    return await host_request("plan.update", payload)


async def request_user_input(questions: list[dict[str, Any]]) -> dict[str, Any]:
    """Ask the user through the active session's input UI."""
    if not isinstance(questions, list):
        raise TypeError("questions must be a list")
    return await host_request("question", {"questions": questions})


# Credential grants (design doc §9): start the fd receiver when the host
# passed a delivery channel (fenced kernels only).
try:
    from . import grants as _grants_mod

    _grants_mod.start_from_env()
except Exception:  # noqa: BLE001 - grants are best-effort native I/O
    _grants_mod = None


def emit(data: dict[str, Any]) -> None:
    """Ship one display event (dict of MIME type -> JSON payload) to the host."""
    from . import repl

    repl.emit(data)


async def read(path, *, encoding="utf-8") -> str:
    """Front-door file read (design doc rlm-permissions.md §6.1).

    Try a direct ``open()`` first: inside the OS fence this always succeeds.
    Only when the OS refuses (``OSError`` — including ``ENOENT`` for paths
    hidden by a Linux mount fence, not just ``EACCES``) does this ask the host
    to read the file through the approval pipeline. The kernel process is
    never restarted. A host denial surfaces as ``PermissionError`` with the
    host's self-explanatory message.
    """
    try:
        with open(path, encoding=encoding) as f:
            return f.read()
    except OSError as direct_error:
        # Delivered credential (§9): a granted dirfd makes this a native
        # kernel-side read — no host round-trip, no restart.
        if _grants_mod is not None:
            grant = _grants_mod.grant_fd_for(os.fspath(path))
            if grant is not None:
                fd, _access, rel = grant
                try:
                    with os.fdopen(os.open(rel, os.O_RDONLY, dir_fd=fd), encoding=encoding) as f:
                        return f.read()
                except OSError:
                    pass  # fall through to the mediated front door
        try:
            reply = await host_request(
                "fs.read", {"path": str(path), "errno": direct_error.errno}
            )
        except RuntimeError as denied:
            raise PermissionError(str(denied)) from direct_error
        content = reply.get("content")
        if not isinstance(content, str):
            raise RuntimeError("fs.read reply carried no content") from direct_error
        return content


async def read_once(path, *, encoding="utf-8") -> str:
    """Ephemeral single-read credential (design doc §6.1/§9): ask once, the
    host approves once, the bytes come back bound to the caller's context.
    Nothing outlives the reply — no ACE, no dirfd, no cache, no restart."""
    try:
        with open(path, encoding=encoding) as f:
            return f.read()
    except OSError as direct_error:
        try:
            reply = await host_request(
                "fs.read",
                {"path": str(path), "errno": direct_error.errno, "once": True},
            )
        except RuntimeError as denied:
            raise PermissionError(str(denied)) from direct_error
        content = reply.get("content")
        if not isinstance(content, str):
            raise RuntimeError("fs.read(once) reply carried no content") from direct_error
        return content


async def write_once(path, content) -> None:
    """Ephemeral single-write (§6.1/§9): one approval, host executes with the
    kernel's context honored, nothing persists (no cache, no ACE)."""
    if not isinstance(content, str):
        raise TypeError(f"content must be str, got {type(content).__name__}")
    try:
        with open(path, "w", encoding="utf-8", newline="") as f:
            f.write(content)
        return
    except OSError as direct_error:
        try:
            await host_request(
                "fs.write",
                {
                    "filePath": str(path),
                    "content": content,
                    "once": True,
                },
            )
            return
        except RuntimeError as denied:
            raise PermissionError(str(denied)) from direct_error


async def write(path, content) -> None:
    """Front-door file write (design doc rlm-permissions.md §6.1).

    Same contract as :func:`read`: direct write inside the fence, host-mediated
    write (approval pipeline) when the OS refuses. ``content`` must be ``str``.
    """
    if not isinstance(content, str):
        raise TypeError(f"content must be str, got {type(content).__name__}")
    try:
        with open(path, "w", encoding="utf-8", newline="") as f:
            f.write(content)
        return
    except OSError as direct_error:
        if _grants_mod is not None:
            grant = _grants_mod.grant_fd_for(os.fspath(path))
            if grant is not None:
                fd, access, rel = grant
                if access == "write":
                    try:
                        fd_open = os.open(
                            rel, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o644, dir_fd=fd
                        )
                        with os.fdopen(fd_open, "w", encoding="utf-8", newline="") as f:
                            f.write(content)
                        return
                    except OSError:
                        pass  # fall through to the mediated front door
        try:
            # `filePath` matches the mediated write tool contract; the bridge's
            # approval-path extraction also accepts `path`/`file_path`.
            await host_request("fs.write", {"filePath": str(path), "content": content})
            return
        except RuntimeError as denied:
            raise PermissionError(str(denied)) from direct_error


async def spawn(
    prompt: str,
    *,
    name: str,
    model: str | None = None,
    thinking: str | None = None,
) -> RLMSpawnHandle:
    """Spawn a recursive Devo child and return once its task is admitted.

    ``name`` is required and must be unique among siblings.
    ``model`` selects a child with an exact ``provider/model`` selector.
    ``thinking`` sets the child reasoning level (e.g. 'off', 'low', 'medium', 'high');
    defaults to the parent level; levels invalid for the resolved model fail the spawn.
    """
    if not isinstance(prompt, str):
        raise TypeError(f"prompt must be str, got {type(prompt).__name__}")
    kwargs: dict[str, Any] = {"name": name}
    if model is not None:
        kwargs["model"] = model
    if thinking is not None:
        kwargs["thinking"] = thinking
    # Wire type stays "rlm.run" so kernels and hosts of different versions stay compatible.
    payload = await host_request("rlm.run", {"prompt": prompt, "kwargs": kwargs})
    return _spawn_handle_from_payload(payload)


def _model_from_payload(payload: Any) -> RLMModel:
    if not isinstance(payload, dict):
        raise RuntimeError("rlm.find_models returned an invalid model entry")
    provider = payload.get("provider")
    model_id = payload.get("id")
    name = payload.get("name")
    selector = payload.get("selector")
    if not all(isinstance(value, str) and value for value in (provider, model_id, name, selector)):
        raise RuntimeError("rlm.find_models returned an invalid model entry")
    return RLMModel(provider=provider, id=model_id, name=name, selector=selector)


async def create_session(
    prompt: str,
    name: str | None = None,
    model: str | None = None,
    thinking: str | None = None,
    cwd: str | None = None,
) -> RLMCreateSessionHandle:
    """Create and prompt a resident depth-0 daemon session.

    Only daemon-backed depth-0 sessions support this operation. The optional
    arguments set the session name, model, thinking level, and working directory.
    """
    if not isinstance(prompt, str):
        raise TypeError(f"prompt must be str, got {type(prompt).__name__}")
    kwargs: dict[str, Any] = {}
    if name is not None:
        kwargs["name"] = name
    if model is not None:
        kwargs["model"] = model
    if thinking is not None:
        kwargs["thinking"] = thinking
    if cwd is not None:
        kwargs["cwd"] = cwd
    payload = await host_request("rlm.create_session", {"prompt": prompt, "kwargs": kwargs})
    return _create_session_handle_from_payload(payload)


async def find_models(query: str = "", limit: int = 8) -> list[RLMModel]:
    """Search a bounded list of models backed by active user credentials."""
    if not isinstance(query, str):
        raise TypeError(f"query must be str, got {type(query).__name__}")
    if not isinstance(limit, int):
        raise TypeError(f"limit must be int, got {type(limit).__name__}")
    payload = await host_request("rlm.find_models", {"query": query, "limit": limit})
    models = payload.get("models")
    if not isinstance(models, list):
        raise RuntimeError("rlm.find_models returned an invalid models list")
    return [_model_from_payload(model) for model in models]


def _subagent_from_payload(payload: Any, operation: str = "rlm.list_subagents") -> RLMSubagent:
    if not isinstance(payload, dict):
        raise RuntimeError(f"{operation} returned an invalid subagent entry")
    child_id = payload.get("rlm_child_id")
    active_session_id = payload.get("active_session_id")
    session_id = payload.get("session_id")
    session_name = payload.get("session_name")
    session_dir = payload.get("session_dir")
    status = payload.get("status")
    if not isinstance(child_id, str) or not child_id:
        raise RuntimeError(f"{operation} entry is missing rlm_child_id")
    if active_session_id is not None and not isinstance(active_session_id, str):
        raise RuntimeError(f"{operation} entry has invalid active_session_id")
    if session_id is not None and not isinstance(session_id, str):
        raise RuntimeError(f"{operation} entry has invalid session_id")
    if not isinstance(session_name, str) or not session_name:
        raise RuntimeError(f"{operation} entry is missing session_name")
    if not isinstance(session_dir, str) or not session_dir:
        raise RuntimeError(f"{operation} entry is missing session_dir")
    if status not in {"running", "completed", "error"}:
        raise RuntimeError(f"{operation} entry has invalid status")
    return RLMSubagent(
        rlm_child_id=child_id,
        active_session_id=active_session_id,
        session_id=session_id,
        session_name=session_name,
        session_dir=Path(session_dir),
        status=status,
    )


async def list_subagents() -> list[RLMSubagent]:
    """List direct RLM children retained by the current parent session."""
    payload = await host_request("rlm.list_subagents")
    entries = payload.get("subagents")
    if not isinstance(entries, list):
        raise RuntimeError("rlm.list_subagents returned an invalid subagents registry")
    return [_subagent_from_payload(entry) for entry in entries]


async def delete_subagent(target: str | RLMSubagent) -> RLMSubagent:
    """Delete one running or retained direct child from the current parent session."""
    if isinstance(target, RLMSubagent):
        selector = target.rlm_child_id
    elif isinstance(target, str):
        selector = target.strip()
        if not selector:
            raise ValueError("target must not be empty")
    else:
        raise TypeError(f"target must be str or RLMSubagent, got {type(target).__name__}")
    payload = await host_request("rlm.delete_subagent", {"target": selector})
    return _subagent_from_payload(payload.get("subagent"), "rlm.delete_subagent")


class _HarnessProxy:
    """Resolve the harness state against the current environment on every access.

    Session env vars may be applied after import, so a state bound at import
    time could freeze an env-less resolution. Resolution must never raise (a
    failure inside the kernel namespace would take down the kernel). When the
    local store is genuinely unconfigured (no session env, e.g. --no-session)
    reads see an empty view but local writes raise instructively instead of
    vanishing on kernel exit; any other resolution failure degrades to a shared
    in-memory store until local resolution starts succeeding.
    """

    _fallback: HarnessState | None = None
    _unpersisted: HarnessState | None = None

    def _resolve(self) -> HarnessState:
        try:
            return get_harness_state()
        except RuntimeError as exc:
            if "Local harness state requires" in str(exc):
                if _HarnessProxy._unpersisted is None:
                    _HarnessProxy._unpersisted = HarnessState(
                        in_memory=True,
                        local_write_error=(
                            f"{exc} This session has no persistent local harness store; "
                            "pass global_=True to persist across sessions."
                        ),
                    )
                return _HarnessProxy._unpersisted
            return self._degraded()
        except Exception:  # pragma: no cover - harness access must never raise
            return self._degraded()

    @staticmethod
    def _degraded() -> HarnessState:
        if _HarnessProxy._fallback is None:
            _HarnessProxy._fallback = HarnessState(in_memory=True)
        return _HarnessProxy._fallback

    def __getattr__(self, name: str) -> Any:
        return getattr(self._resolve(), name)

    def __repr__(self) -> str:
        return repr(self._resolve())


_harness_state = _HarnessProxy()


class _RLMNamespace:
    harness = _harness_state
    get_harness_state = staticmethod(get_harness_state)

    async def spawn(
        self,
        prompt: str,
        *,
        name: str,
        model: str | None = None,
        thinking: str | None = None,
    ) -> RLMSpawnHandle:
        return await spawn(prompt, name=name, model=model, thinking=thinking)

    async def create_session(
        self,
        prompt: str,
        name: str | None = None,
        model: str | None = None,
        thinking: str | None = None,
        cwd: str | None = None,
    ) -> RLMCreateSessionHandle:
        return await create_session(prompt, name=name, model=model, thinking=thinking, cwd=cwd)

    async def find_models(self, query: str = "", limit: int = 8) -> list[RLMModel]:
        return await find_models(query, limit)

    async def web_search(self, query: str) -> dict[str, Any]:
        return await web_search(query)

    async def web_fetch(
        self, url: str, *, format: str | None = None, timeout: int | None = None
    ) -> dict[str, Any]:
        return await web_fetch(url, format=format, timeout=timeout)

    async def update_plan(
        self, plan: list[dict[str, str]], *, explanation: str | None = None
    ) -> dict[str, Any]:
        return await update_plan(plan, explanation=explanation)

    async def request_user_input(self, questions: list[dict[str, Any]]) -> dict[str, Any]:
        return await request_user_input(questions)

    async def list_subagents(self) -> list[RLMSubagent]:
        return await list_subagents()

    async def delete_subagent(self, target: str | RLMSubagent) -> RLMSubagent:
        return await delete_subagent(target)

    def __call__(self, *args: Any, **kwargs: Any) -> Any:
        raise TypeError(_NOT_CALLABLE_MESSAGE)

    # AttributeError keeps hasattr() semantics intact while still naming the replacement.
    def __getattr__(self, name: str) -> Any:
        if name == "run":
            raise AttributeError(_RENAMED_RUN_MESSAGE)
        raise AttributeError(f"'rlm' object has no attribute {name!r}")


rlm = _RLMNamespace()
harness = _harness_state


class _NotCallableModule(types.ModuleType):
    def __call__(self, *args: Any, **kwargs: Any) -> Any:
        raise TypeError(_NOT_CALLABLE_MESSAGE)


sys.modules[__name__].__class__ = _NotCallableModule

__all__ = [
    "BashHandle",
    "BashResult",
    "HarnessEntry",
    "HarnessScope",
    "HarnessState",
    "McpIntegration",
    "McpToolError",
    "NotEnabled",
    "RLMCreateSessionHandle",
    "RLMModel",
    "RLMSpawnHandle",
    "RLMSubagent",
    "create_session",
    "RefinementEvent",
    "bash",
    "delete_subagent",
    "emit",
    "find_models",
    "get_harness_state",
    "harness",
    "host_request",
    "list_subagents",
    "request_user_input",
    "rlm",
    "spawn",
    "update_plan",
    "web_fetch",
    "web_search",
]

# Lazily re-export the MCP base class. Kept lazy so `import rlm` never requires
# the optional `mcp` SDK — only integration packages that subclass it do.
_LAZY_MCP = {"McpIntegration", "McpToolError", "NotEnabled"}


def __getattr__(name: str) -> Any:  # noqa: D401 - module-level lazy attr hook
    if name in _LAZY_MCP:
        from . import mcp_base

        return getattr(mcp_base, name)
    if name == "run":
        raise AttributeError(_RENAMED_RUN_MESSAGE)
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")

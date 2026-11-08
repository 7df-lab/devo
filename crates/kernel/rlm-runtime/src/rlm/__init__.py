"""Tiny rlm-compatible kernel shim for Devo."""

from __future__ import annotations

import sys
import types
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from .bash import BashHandle, BashResult, bash
from .shell import shell
from .harness import HarnessEntry, HarnessScope, HarnessState, RefinementEvent, get_harness_state
from .factory import FactoryNamespace

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
    thinking_levels: list[str] = field(default_factory=list)


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


def emit(data: dict[str, Any]) -> None:
    """Ship one display event (dict of MIME type -> JSON payload) to the host."""
    from . import repl

    repl.emit(data)


async def read(path, *, encoding="utf-8") -> str:
    """Front-door file read (design doc rlm-permissions.md §6.1).

    Try a direct ``open()`` with the user's OS privileges first. On
    ``OSError``, ask the host to read through the approval pipeline.
    The kernel process is
    never restarted. A host denial surfaces as ``PermissionError`` with the
    host's self-explanatory message.
    """
    try:
        with open(path, encoding=encoding) as f:
            return f.read()
    except OSError as direct_error:
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
    """Read directly, or request a one-time host-mediated read on OS failure."""
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
    """Write directly, or request a one-time host-mediated write on OS failure."""
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

    Same contract as :func:`read`: direct write with user privileges, host-mediated
    write (approval pipeline) when the OS refuses. ``content`` must be ``str``.
    """
    if not isinstance(content, str):
        raise TypeError(f"content must be str, got {type(content).__name__}")
    try:
        with open(path, "w", encoding="utf-8", newline="") as f:
            f.write(content)
        return
    except OSError as direct_error:
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
    ``model`` selects a child with an exact ``provider/model`` selector from
    ``find_models`` (model IDs may contain slashes); there is no fallback.
    ``thinking`` sets the child reasoning level (e.g. 'off', 'low', 'medium', 'high');
    Omitted or None options inherit the immediate parent's model and thinking.
    Levels invalid for the resolved model, including inherited levels, fail the spawn.
    """
    if not isinstance(prompt, str):
        raise TypeError(f"prompt must be str, got {type(prompt).__name__}")
    if not prompt.strip():
        raise ValueError("prompt must be a non-empty string")
    for label, value in (("name", name), ("model", model), ("thinking", thinking)):
        if value is None and label != "name":
            continue
        if not isinstance(value, str):
            raise TypeError(f"{label} must be str, got {type(value).__name__}")
        if not value.strip():
            raise ValueError(f"{label} must be a non-empty string")
    if model is not None:
        provider, separator, model_id = model.partition("/")
        if not separator or not provider.strip() or not model_id.strip():
            raise ValueError("model must be an exact provider/model selector")
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
    thinking_levels = payload.get("thinking_levels", [])
    if not isinstance(thinking_levels, list) or not all(
        isinstance(level, str) and level.strip() for level in thinking_levels
    ):
        raise RuntimeError("rlm.find_models returned invalid thinking_levels")
    return RLMModel(
        provider=provider, id=model_id, name=name, selector=selector,
        thinking_levels=thinking_levels,
    )


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
    """Search enabled, configured, routable models without network probes.

    Availability describes local configuration, not live health or credit. Each
    descriptor includes supported thinking levels; older hosts may omit them.
    ``limit`` must be an integer from 1 to 100 (not bool).
    """
    if not isinstance(query, str):
        raise TypeError(f"query must be str, got {type(query).__name__}")
    if isinstance(limit, bool) or not isinstance(limit, int):
        raise TypeError(f"limit must be int, got {type(limit).__name__}")
    if not 1 <= limit <= 100:
        raise ValueError("limit must be between 1 and 100")
    payload = await host_request("rlm.find_models", {"query": query, "limit": limit})
    if not isinstance(payload, dict):
        raise RuntimeError("rlm.find_models returned an invalid models list")
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
    """Lazily expose the host-bound local view without filesystem resolution.

    Construction does no I/O. Reads inspect the cached view; await load() to
    refresh, and await each mutation. Host failures never become volatile writes.
    """

    def __getattr__(self, name: str) -> Any:
        return getattr(get_harness_state(), name)

    def __repr__(self) -> str:
        return repr(get_harness_state())


_harness_state = _HarnessProxy()
factory = FactoryNamespace()


class _RLMNamespace:
    shell = staticmethod(shell)
    harness = _harness_state
    factory = factory
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
    "FactoryNamespace",
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
    "shell",
    "delete_subagent",
    "emit",
    "find_models",
    "factory",
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

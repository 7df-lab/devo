"""Typed, bounded Python orchestration over the host's session lifecycle."""
from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Any


@dataclass(frozen=True)
class RLMChildResult:
    rlm_child_id: str
    session_name: str
    session_dir: Path | None
    status: str
    settled: bool
    answer_text: str | None = None
    answer_preview: str | None = None
    error: str | None = None
    progress_note: str | None = None
    answer_truncated: bool = False
    requires_resume: bool = False


@dataclass(frozen=True)
class RLMProgressNoteResult:
    accepted: bool
    retry_after_ms: int | None = None


async def collect(targets=None, *, timeout_ms: int = 0) -> list[RLMChildResult]:
    """Snapshot direct children, waiting at most 300000 ms for all to settle.

    Accepts a name, spawn handle, subagent row, or a list/tuple of these.
    None/[] snapshots all direct children at admission. Timeout returns partial
    snapshots, never cancels children or consumes their retained results.
    """
    from . import RLMSpawnHandle, RLMSubagent, host_request
    if isinstance(timeout_ms, bool) or not isinstance(timeout_ms, int):
        raise TypeError("timeout_ms must be an int")
    if not 0 <= timeout_ms <= 300000:
        raise ValueError("timeout_ms must be between 0 and 300000")
    if targets is None:
        targets = []
    elif isinstance(targets, (str, RLMSpawnHandle, RLMSubagent)):
        targets = [targets]
    elif not isinstance(targets, (list, tuple)):
        raise TypeError("targets must be a target or a list/tuple of targets")
    selectors = []
    for target in targets:
        if isinstance(target, (RLMSpawnHandle, RLMSubagent)):
            target = target.rlm_child_id
        if not isinstance(target, str):
            raise TypeError("each target must be a name, spawn handle, or subagent")
        if not target.strip():
            raise ValueError("target must not be empty")
        selectors.append(target.strip())
    payload = await host_request("rlm.collect", {"targets": selectors, "timeout_ms": timeout_ms})
    if not isinstance(payload, dict):
        raise RuntimeError("rlm.collect returned an invalid payload")
    entries = payload.get("results")
    if not isinstance(entries, list):
        raise RuntimeError("rlm.collect returned an invalid results list")
    results = []
    for entry in entries:
        if not isinstance(entry, dict):
            raise RuntimeError("rlm.collect returned an invalid entry")
        if any(not isinstance(entry.get(k), str) or not entry[k] for k in ("rlm_child_id", "session_name", "status")):
            raise RuntimeError("rlm.collect entry is missing identity/status")
        if not isinstance(entry.get("settled"), bool):
            raise RuntimeError("rlm.collect entry has invalid settled flag")
        for key in ("answer_truncated", "requires_resume"):
            if not isinstance(entry.get(key, False), bool):
                raise RuntimeError(f"rlm.collect entry has invalid {key} flag")
        for key in ("session_dir", "answer_text", "answer_preview", "error", "progress_note"):
            if entry.get(key) is not None and not isinstance(entry[key], str):
                raise RuntimeError(f"rlm.collect entry has invalid {key}")
        results.append(RLMChildResult(
            rlm_child_id=entry["rlm_child_id"], session_name=entry["session_name"],
            status=entry["status"], settled=entry["settled"],
            session_dir=Path(entry["session_dir"]) if entry.get("session_dir") else None,
            answer_text=entry.get("answer_text"), answer_preview=entry.get("answer_preview"),
            error=entry.get("error"), progress_note=entry.get("progress_note"),
            answer_truncated=entry.get("answer_truncated", False),
            requires_resume=entry.get("requires_resume", False),
        ))
    return results


async def progress_note(message: str) -> RLMProgressNoteResult:
    """Update a child's progress without steering its parent; throttled to 10s."""
    from . import host_request
    if not isinstance(message, str):
        raise TypeError("message must be str")
    message = message.strip()
    if not message or len(message.encode("utf-16-le", "surrogatepass")) // 2 > 512:
        raise ValueError("message must contain 1 to 512 UTF-16 code units")
    payload = await host_request("rlm.progress.note", {"message": message})
    if not isinstance(payload, dict):
        raise RuntimeError("rlm.progress.note returned an invalid payload")
    accepted, retry = payload.get("accepted"), payload.get("retry_after_ms")
    if not isinstance(accepted, bool) or (retry is not None and (isinstance(retry, bool) or not isinstance(retry, int) or retry < 0)):
        raise RuntimeError("rlm.progress.note returned an invalid result")
    return RLMProgressNoteResult(accepted, retry)


async def end_turn() -> dict[str, Any]:
    """Request normal completion after this tool batch, without canceling children.

    The remainder of this cell and other calls in the batch still execute. No
    additional model response is requested. Idle/obsolete host bridges are rejected.
    """
    from . import host_request
    return await host_request("rlm.end_turn", {})

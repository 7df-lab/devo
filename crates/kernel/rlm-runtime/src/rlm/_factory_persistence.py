"""Host-backed checkpoint transport and fail-closed recovery validation."""

from __future__ import annotations

import json
import re
from typing import Any, Protocol

from ._factory_spec import MAX_OUTPUT_CHARS, validate

MAX_SNAPSHOT_BYTES = 1024 * 1024
_RUN_ID = re.compile(r"[a-f0-9]{32}\Z")
NODE_STATUSES = {"pending", "admitting", "running", "waiting_for_input", "waiting_for_resume",
                 "completed", "failed", "blocked", "cancelled", "skipped", "recovery_required"}
RUN_STATUSES = {"ready", "running", "waiting_for_resume", "stopping", "completed", "failed", "stopped",
                "recovering", "recovery_required", "persistence_error"}


class FactoryStore(Protocol):
    """Atomic session-owned snapshot store with optimistic revision checks.

    Save must return only after durable replacement, reject stale revisions, and
    never silently create a volatile fallback. Load/list must not mutate files.
    A save error may have committed; callers must reload rather than retry blind.
    Spawn/cancel must fence the child operation with save's ownership transaction,
    validating the revision and saved node identity and consuming a revision before
    the effect. The transaction must remain held until the child operation ends.
    """

    async def save(self, run_id: str, expected_revision: int, snapshot: dict[str, Any]) -> int: ...

    async def spawn(self, run_id: str, revision: int, node_id: str, prompt: str) -> tuple[str, int]: ...

    async def cancel(self, run_id: str, revision: int, node_id: str) -> int: ...

    async def load(self, run_id: str) -> dict[str, Any]: ...

    async def list(self) -> list[dict[str, Any]]: ...


class HostFactoryStore:
    async def save(self, run_id: str, expected_revision: int, snapshot: dict[str, Any]) -> int:
        from . import host_request

        if len(json.dumps(snapshot, allow_nan=False).encode("utf-8")) > MAX_SNAPSHOT_BYTES:
            raise ValueError("durable Factory snapshot exceeds 1 MiB")
        result = await host_request("factory.save", {
            "run_id": run_id, "expected_revision": expected_revision, "snapshot": snapshot})
        if (result.get("run_id") != run_id or type(result.get("revision")) is not int
                or result["revision"] != expected_revision + 1):
            raise RuntimeError("factory.save returned an invalid acknowledgement; reload before continuing")
        return result["revision"]

    async def spawn(self, run_id: str, revision: int, node_id: str, prompt: str) -> tuple[str, int]:
        result = await self._effect("spawn", run_id, revision, node_id, prompt=prompt)
        child_id = result.get("child_id")
        if not isinstance(child_id, str) or not child_id:
            raise RuntimeError("factory.spawn returned an invalid child identity; restore before continuing")
        return child_id, result["revision"]

    async def cancel(self, run_id: str, revision: int, node_id: str) -> int:
        result = await self._effect("cancel", run_id, revision, node_id)
        return result["revision"]

    async def _effect(self, operation: str, run_id: str, revision: int, node_id: str, **kwargs) -> dict[str, Any]:
        from . import host_request

        result = await host_request(f"factory.{operation}", {
            "run_id": run_id, "expected_revision": revision, "node_id": node_id, **kwargs})
        if (result.get("run_id") != run_id or type(result.get("revision")) is not int
                or result["revision"] != revision + 1):
            raise RuntimeError("Factory effect returned an invalid acknowledgement; restore before continuing")
        if "error" in result:
            raise RuntimeError(f"Factory effect failed: {result['error']}; restore before continuing")
        return result

    async def load(self, run_id: str) -> dict[str, Any]:
        from . import host_request

        return await host_request("factory.load", {"run_id": run_id})

    async def list(self) -> list[dict[str, Any]]:
        from . import host_request

        result = await host_request("factory.list", {})
        if not isinstance(result.get("runs"), list):
            raise RuntimeError("factory.list returned an invalid run list")
        return result["runs"]


def validate_record(record: Any, run_id: str) -> tuple[Any, dict[str, Any], int]:
    """Validate recovery data without granting it authority over host children."""
    if not isinstance(run_id, str) or not _RUN_ID.fullmatch(run_id):
        raise ValueError("Factory run id must contain 32 lowercase hex characters")
    if not isinstance(record, dict) or record.get("run_id") != run_id:
        raise ValueError("checkpoint belongs to a different run")
    revision = record.get("revision")
    if type(revision) is not int or revision < 1:
        raise ValueError("checkpoint has an invalid revision")
    snapshot = record.get("snapshot")
    if not isinstance(snapshot, dict) or set(snapshot) != {"format_version", "spec", "run"}:
        raise ValueError("checkpoint has an invalid envelope")
    if type(snapshot["format_version"]) is not int or snapshot["format_version"] != 1:
        raise ValueError("unsupported checkpoint format version")
    if len(json.dumps(snapshot, allow_nan=False).encode("utf-8")) > MAX_SNAPSHOT_BYTES:
        raise ValueError("checkpoint exceeds 1 MiB")
    spec = validate(snapshot["spec"])
    state = snapshot["run"]
    if not isinstance(state, dict) or set(state) != {"id", "status", "stop_requested", "failed", "nodes"}:
        raise ValueError("checkpoint has invalid run fields")
    if state["id"] != run_id or state["status"] not in RUN_STATUSES:
        raise ValueError("checkpoint has invalid run identity/status")
    if type(state["stop_requested"]) is not bool or type(state["failed"]) is not bool:
        raise ValueError("checkpoint has invalid run flags")
    nodes = state["nodes"]
    if not isinstance(nodes, dict) or set(nodes) != {node.id for node in spec.nodes}:
        raise ValueError("checkpoint nodes do not match its definition")
    child_ids: set[str] = set()
    for node_id, node in nodes.items():
        if not isinstance(node, dict) or set(node) != {
            "id", "name", "status", "child_id", "output", "error", "cleanup_error", "needs_cleanup"
        }:
            raise ValueError("checkpoint has invalid node fields")
        if node["id"] != node_id or node["name"] != f"factory-{run_id[:16]}-{node_id}":
            raise ValueError("checkpoint has an invalid reserved child name")
        if node["status"] not in NODE_STATUSES or type(node["needs_cleanup"]) is not bool:
            raise ValueError("checkpoint has an invalid node status/cleanup flag")
        for key in ("child_id", "output", "error", "cleanup_error"):
            if node[key] is not None and not isinstance(node[key], str):
                raise ValueError(f"checkpoint has an invalid node {key}")
        if node["child_id"] is not None and not node["child_id"].strip():
            raise ValueError("checkpoint has an empty child id")
        if node["child_id"] is not None:
            if node["child_id"] in child_ids:
                raise ValueError("checkpoint binds multiple nodes to the same child")
            child_ids.add(node["child_id"])
        if node["output"] is not None and len(node["output"]) > MAX_OUTPUT_CHARS:
            raise ValueError("checkpoint output exceeds the node limit")
        if node["status"] in {"pending", "blocked", "skipped"} and (node["child_id"] is not None or node["needs_cleanup"]):
            raise ValueError("checkpoint cannot make an admitted child pending, blocked or skipped")
        if node["status"] in {"admitting", "running", "waiting_for_input", "waiting_for_resume", "recovery_required"} and not node["needs_cleanup"]:
            raise ValueError("checkpoint cannot drop cleanup for an unresolved child")
        if node["status"] in {"running", "waiting_for_input", "waiting_for_resume", "completed"} and node["child_id"] is None:
            raise ValueError("checkpoint lost an admitted child identity")
        if node["status"] == "completed" and (not isinstance(node["output"], str) or node["needs_cleanup"]):
            raise ValueError("checkpoint lost a completed output or still needs cleanup")
        if node["status"] == "failed" and not state["failed"]:
            raise ValueError("checkpoint cannot hide a failed node")
    if state["status"] in {"completed", "failed", "stopped"} and any(
        node["needs_cleanup"] or node["status"] == "pending" for node in nodes.values()
    ):
        raise ValueError("terminal checkpoint still has unresolved work")
    if state["status"] == "completed" and (state["failed"] or state["stop_requested"] or any(
        node["status"] not in {"completed", "skipped"} for node in nodes.values()
    )):
        raise ValueError("completed checkpoint has inconsistent terminal state")
    if state["status"] == "failed" and not state["failed"]:
        raise ValueError("failed checkpoint lost its failure flag")
    if state["status"] == "stopped" and (state["failed"] or not state["stop_requested"]):
        raise ValueError("stopped checkpoint has inconsistent flags")
    return spec, state, revision

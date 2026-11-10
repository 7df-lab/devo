"""Strict, side-effect-free validation for the first Devo Factory DAG format."""

from __future__ import annotations

import re
import math
from dataclasses import dataclass
from typing import Any

MAX_NODES = 128
MAX_PARALLEL = 16
MAX_PROMPT_CHARS = 128_000
MAX_OUTPUT_CHARS = 32_000
_ID = re.compile(r"[a-z][a-z0-9_-]{0,31}\Z")


@dataclass(frozen=True)
class FactoryGuard:
    node: str
    equals: str | int | float | bool | None
    path: tuple[str, ...] | None = None


@dataclass(frozen=True)
class FactoryNode:
    id: str
    prompt: str
    needs: tuple[str, ...] = ()
    model: str | None = None
    thinking: str | None = None
    join: str = "all"
    when: FactoryGuard | None = None


@dataclass(frozen=True)
class FactorySpec:
    nodes: tuple[FactoryNode, ...]
    max_parallel: int = 4
    failure_policy: str = "fail_fast"
    version: int = 1


def validate(spec: Any) -> FactorySpec:
    """Return an immutable DAG; reject unsupported options rather than ignore them.

    Version 1 is intentionally not a Prime machine specification. It supports
    finite DAGs only, with explicit dependencies and plain-text child outputs.
    """
    if not isinstance(spec, dict):
        raise ValueError("factory spec must be an object")
    unknown = set(spec) - {"version", "nodes", "max_parallel", "failure_policy"}
    if unknown:
        raise ValueError(f"unsupported factory fields: {sorted(unknown, key=str)}")
    if type(spec.get("version")) is not int or spec["version"] not in (1, 2):
        raise ValueError("factory version must be 1 or 2")
    max_parallel = spec.get("max_parallel", 4)
    if type(max_parallel) is not int or not 1 <= max_parallel <= MAX_PARALLEL:
        raise ValueError(f"max_parallel must be an integer from 1 to {MAX_PARALLEL}")
    failure_policy = spec.get("failure_policy", "fail_fast")
    if failure_policy not in ("fail_fast", "continue"):
        raise ValueError("failure_policy must be fail_fast or continue")
    raw_nodes = spec.get("nodes")
    if not isinstance(raw_nodes, list) or not 1 <= len(raw_nodes) <= MAX_NODES:
        raise ValueError(f"nodes must contain 1 to {MAX_NODES} nodes")
    nodes: list[FactoryNode] = []
    seen: set[str] = set()
    for raw in raw_nodes:
        if not isinstance(raw, dict):
            raise ValueError("each factory node must be an object")
        allowed = {"id", "prompt", "needs", "model", "thinking"}
        if spec["version"] == 2:
            allowed |= {"join", "when"}
        unknown = set(raw) - allowed
        if unknown:
            raise ValueError(f"unsupported node fields: {sorted(unknown, key=str)}")
        node_id = raw.get("id")
        if not isinstance(node_id, str) or not _ID.fullmatch(node_id):
            raise ValueError("node id must match [a-z][a-z0-9_-]{0,31}")
        if node_id in seen:
            raise ValueError(f"duplicate node id: {node_id}")
        seen.add(node_id)
        prompt = raw.get("prompt")
        if not isinstance(prompt, str) or not prompt.strip() or len(prompt) > MAX_PROMPT_CHARS:
            raise ValueError(f"node {node_id}: prompt must contain 1 to {MAX_PROMPT_CHARS} characters")
        needs = raw.get("needs", [])
        if not isinstance(needs, list) or not all(isinstance(item, str) for item in needs):
            raise ValueError(f"node {node_id}: needs must be a list of node ids")
        if len(set(needs)) != len(needs):
            raise ValueError(f"node {node_id}: duplicate dependency")
        model = raw.get("model")
        if model is not None:
            if not isinstance(model, str):
                raise ValueError(f"node {node_id}: model must be a provider/model selector")
            provider, separator, model_id = model.partition("/")
            if not separator or not provider.strip() or not model_id.strip():
                raise ValueError(f"node {node_id}: model must be a provider/model selector")
        thinking = raw.get("thinking")
        if thinking is not None and (not isinstance(thinking, str) or not thinking.strip()):
            raise ValueError(f"node {node_id}: thinking must be a nonempty string")
        join = raw.get("join", "all")
        if join not in ("all", "any") or (join == "any" and not needs):
            raise ValueError(f"node {node_id}: join must be all, or any with dependencies")
        guard = None
        if "when" in raw:
            when = raw["when"]
            if not isinstance(when, dict) or set(when) - {"node", "path", "equals"} or not {"node", "equals"} <= set(when):
                raise ValueError(f"node {node_id}: when requires node/equals and optional path")
            source, equals = when["node"], when["equals"]
            if not isinstance(source, str) or source not in needs:
                raise ValueError(f"node {node_id}: guard source must be an explicit dependency")
            if equals is not None and type(equals) not in (str, bool, int, float):
                raise ValueError(f"node {node_id}: guard equals must be a finite JSON scalar")
            if isinstance(equals, float) and not math.isfinite(equals):
                raise ValueError(f"node {node_id}: guard equals must be finite")
            if type(equals) is int and abs(equals) > 2**53 - 1:
                raise ValueError(f"node {node_id}: guard integer must be in the JSON safe integer range")
            path = when.get("path")
            if "path" in when and (not isinstance(path, list) or len(path) > 16
                                    or not all(isinstance(key, str) and key for key in path)):
                raise ValueError(f"node {node_id}: guard path must contain at most 16 nonempty object keys")
            if path is None and not isinstance(equals, str):
                raise ValueError(f"node {node_id}: text guard equals must be a string")
            guard = FactoryGuard(source, equals, tuple(path) if path is not None else None)
        nodes.append(FactoryNode(node_id, prompt, tuple(needs), model, thinking, join, guard))
    for node in nodes:
        missing = set(node.needs) - seen
        if missing:
            raise ValueError(f"node {node.id}: unknown dependencies: {sorted(missing)}")
    remaining = {node.id: set(node.needs) for node in nodes}
    while remaining:
        ready = {node_id for node_id, needs in remaining.items() if not needs}
        if not ready:
            raise ValueError("factory dependencies must not contain a cycle")
        remaining = {node_id: needs - ready for node_id, needs in remaining.items() if node_id not in ready}
    return FactorySpec(tuple(nodes), max_parallel, failure_policy, spec["version"])


def spec_dict(spec: FactorySpec) -> dict[str, Any]:
    """Serialize a canonical spec using only the public, versioned fields."""
    nodes = []
    for node in spec.nodes:
        raw = {"id": node.id, "prompt": node.prompt, "needs": list(node.needs)}
        for key in ("model", "thinking"):
            if getattr(node, key) is not None:
                raw[key] = getattr(node, key)
        if spec.version == 2:
            raw["join"] = node.join
            if node.when is not None:
                raw["when"] = {"node": node.when.node, "equals": node.when.equals}
                if node.when.path is not None:
                    raw["when"]["path"] = list(node.when.path)
        nodes.append(raw)
    return {"version": spec.version, "nodes": nodes, "max_parallel": spec.max_parallel,
            "failure_policy": spec.failure_policy}

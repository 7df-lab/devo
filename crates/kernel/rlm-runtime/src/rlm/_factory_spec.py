"""Strict, side-effect-free validation for the first Devo Factory DAG format."""

from __future__ import annotations

import re
from dataclasses import dataclass
from typing import Any

MAX_NODES = 128
MAX_PARALLEL = 16
MAX_PROMPT_CHARS = 128_000
MAX_OUTPUT_CHARS = 32_000
_ID = re.compile(r"[a-z][a-z0-9_-]{0,31}\Z")


@dataclass(frozen=True)
class FactoryNode:
    id: str
    prompt: str
    needs: tuple[str, ...] = ()
    model: str | None = None
    thinking: str | None = None


@dataclass(frozen=True)
class FactorySpec:
    nodes: tuple[FactoryNode, ...]
    max_parallel: int = 4
    failure_policy: str = "fail_fast"


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
    if type(spec.get("version")) is not int or spec["version"] != 1:
        raise ValueError("factory version must be 1")
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
        unknown = set(raw) - {"id", "prompt", "needs", "model", "thinking"}
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
        nodes.append(FactoryNode(node_id, prompt, tuple(needs), model, thinking))
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
    return FactorySpec(tuple(nodes), max_parallel, failure_policy)

"""An isolated adapter to host-owned child lifecycle, never direct session I/O."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Protocol

from ._factory_spec import FactoryNode


@dataclass(frozen=True)
class ChildSnapshot:
    child_id: str
    status: str
    settled: bool
    output: str = ""
    error: str | None = None
    requires_resume: bool = False


class FactoryBackend(Protocol):
    """Host boundary for Factory; implementations must preserve parent permissions.

    Spawn admits one ordinary direct child. Inspect is non-consuming and must
    return exactly the requested children or raise, never infer completion from
    an absent child. Cancel confirms termination, not just signal delivery.
    A failed or cancelled spawn has an uncertain outcome; the executor retains
    its unique name for cancellation and never silently retries the admission.
    """

    def check_available(self) -> None: ...

    async def spawn(self, node: FactoryNode, prompt: str, name: str) -> str: ...

    async def inspect(self, child_ids: list[str]) -> list[ChildSnapshot]: ...

    async def cancel(self, target: str) -> None: ...

    async def resolve(self, name: str) -> ChildSnapshot | None: ...


class RlmFactoryBackend:
    """Use only the public RLM helpers and their existing Rust authorization."""

    def check_available(self) -> None:
        import rlm

        if not callable(getattr(rlm, "collect", None)):
            raise RuntimeError(
                "Factory requires the RLM child collection API (rlm.collect); "
                "install the subagent-controls implementation before starting a run"
            )

    async def spawn(self, node: FactoryNode, prompt: str, name: str) -> str:
        import rlm

        handle = await rlm.spawn(prompt, name=name, model=node.model, thinking=node.thinking)
        return handle.rlm_child_id

    async def inspect(self, child_ids: list[str]) -> list[ChildSnapshot]:
        import rlm

        results = await rlm.collect(targets=child_ids, timeout_ms=0)
        return [ChildSnapshot(item.rlm_child_id, item.status, item.settled,
                              item.answer_text or "", item.error,
                              getattr(item, "requires_resume", False)) for item in results]

    async def cancel(self, target: str) -> None:
        import rlm

        await rlm.delete_subagent(target)

    async def resolve(self, name: str) -> ChildSnapshot | None:
        import rlm

        matches = [item for item in await rlm.list_subagents() if item.session_name == name]
        if len(matches) > 1:
            raise RuntimeError("reserved Factory child name is ambiguous")
        if not matches:
            return None
        snapshots = await self.inspect([matches[0].rlm_child_id])
        if len(snapshots) != 1 or snapshots[0].child_id != matches[0].rlm_child_id:
            raise RuntimeError("reserved Factory child could not be reconciled")
        return snapshots[0]

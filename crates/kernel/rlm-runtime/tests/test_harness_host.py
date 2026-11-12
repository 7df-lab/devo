"""Contract tests for the host-owned harness facade (no real filesystem I/O)."""

import unittest
from dataclasses import asdict
from importlib import import_module
from unittest.mock import AsyncMock, patch

import rlm

harness = import_module("rlm.harness")


def snapshot(entry=None):
    entries = {kind: {} for kind in ("prompt", "memory", "skill", "subagent")}
    if entry is not None:
        entries[entry.kind][entry.id] = asdict(entry)
    return {"schema": 1, "entries": entries, "refinements": []}


class HarnessHostTests(unittest.IsolatedAsyncioTestCase):
    def test_overview_exposes_self_steering_and_fifo_limits(self):
        overview = harness.HarnessState().overview()
        self.assertIn("receiver_role='self'", overview)
        self.assertIn("next safe model/tool boundary", overview)
        self.assertIn("Strict FIFO prevents overtaking earlier follow-up or held input", overview)
        self.assertIn("Queued means admitted, not processed", overview)
        self.assertIn("never human authorization", overview)

    async def test_construction_is_lazy_and_load_is_explicit(self):
        entry = harness.HarnessEntry(id="note", kind="memory", title="Note", content="text")
        request = AsyncMock(return_value={"state": snapshot(entry), "file_path": None})
        with patch.object(rlm, "host_request", request), patch("builtins.open", side_effect=AssertionError("disk access")):
            state = harness.HarnessState()
            self.assertEqual(state.list(), [])
            request.assert_not_called()
            self.assertIs(await state.load(), state)
            self.assertEqual(state.get("memory", "note"), entry)
            self.assertIn("[local:note]", state.overview())
        request.assert_awaited_once_with("harness.load", {"scope": "local"})

    async def test_mutation_sends_operation_and_refreshes_view(self):
        entry = harness.HarnessEntry(id="note", kind="memory", title="Note", content="text", version=7)
        request = AsyncMock(return_value={"state": snapshot(entry), "entry": asdict(entry)})
        state = harness.HarnessState()
        with patch.object(rlm, "host_request", request):
            created = await state.create_memory("Note", "text")
        self.assertEqual(created, entry)
        self.assertEqual(state.list(), [entry])
        request.assert_awaited_once_with("harness.create", {
            "scope": "local", "kind": "memory", "id": "note", "title": "Note",
            "content": "text", "path": "general", "reference": None,
            "arguments": None, "metadata": None, "source": "agent",
        })

    async def test_update_does_not_send_a_stale_snapshot(self):
        entry = harness.HarnessEntry(id="note", kind="memory", title="New", content="new", version=9)
        request = AsyncMock(return_value={"state": snapshot(entry), "entry": asdict(entry)})
        state = harness.HarnessState()
        with patch.object(rlm, "host_request", request):
            self.assertEqual(await state.update_memory("note", "New", "new"), entry)
        self.assertEqual(request.await_args.args, ("harness.update", {
            "scope": "local", "kind": "memory", "id": "note", "title": "New",
            "content": "new", "path": None, "reference": None,
            "arguments": None, "metadata": None, "source": "agent",
        }))

    async def test_host_denial_keeps_last_view_and_never_falls_back(self):
        state = harness.HarnessState()
        request = AsyncMock(side_effect=RuntimeError("denied: harness.create blocked in Plan mode"))
        with patch.object(rlm, "host_request", request):
            with self.assertRaisesRegex(RuntimeError, "Plan mode"):
                await state.create_memory("Note", "text")
        self.assertEqual(state.snapshot()["entries"], snapshot()["entries"])
        self.assertFalse(state._in_memory)

    async def test_global_prefix_routes_to_global_host_store(self):
        entry = harness.HarnessEntry(id="note", kind="memory", title="Note", content="text", scope="global")
        request = AsyncMock(return_value={"state": snapshot(entry), "entry": asdict(entry)})
        with patch.dict(harness._state_cache, {}, clear=True), patch.object(rlm, "host_request", request):
            state = harness.get_harness_state()
            self.assertEqual(await state.create_memory("Note", "text", id="global:note"), entry)
            self.assertEqual(state.list(), [])
            self.assertEqual(state.list(global_=True), [entry])
        self.assertEqual(request.await_args.args[1]["scope"], "global")
        self.assertEqual(request.await_args.args[1]["id"], "note")

    async def test_delete_and_refinement_use_typed_requests(self):
        state = harness.HarnessState()
        event = harness.RefinementEvent(id="r1", trigger="review", changes=["a"])
        event_state = snapshot()
        event_state["refinements"] = [asdict(event)]
        request = AsyncMock(side_effect=[
            {"state": snapshot(), "deleted": False},
            {"state": event_state, "event": asdict(event)},
        ])
        with patch.object(rlm, "host_request", request):
            self.assertFalse(await state.delete_memory("missing"))
            self.assertEqual(await state.record_refinement("review", "a", id="r1"), event)
        self.assertEqual(request.await_args_list[0].args, ("harness.delete", {
            "scope": "local", "kind": "memory", "id": "missing",
        }))
        self.assertEqual(request.await_args_list[1].args, ("harness.record_refinement", {
            "scope": "local", "trigger": "review", "changes": ["a"], "evidence": "", "outcome": "", "id": "r1",
        }))

    async def test_in_memory_mode_remains_explicit_and_volatile(self):
        state = harness.HarnessState(in_memory=True)
        request = AsyncMock(side_effect=AssertionError("unexpected host request"))
        with patch.object(rlm, "host_request", request):
            created = await state.create_memory("Note", "first")
            self.assertEqual(created.version, 1)
            updated = await state.update_memory("note", "Note", "second")
            self.assertEqual(updated.version, 2)
            with self.assertRaisesRegex(ValueError, "already exists"):
                await state.create_memory("Note", "duplicate")
            self.assertTrue(await state.delete_memory("note"))
            self.assertIs(state.save(), state)
            self.assertIs(await state.load(), state)
        self.assertEqual(state.list(), [])
        request.assert_not_called()

    def test_persistent_snapshot_save_and_path_overrides_fail_helpfully(self):
        with self.assertRaisesRegex(RuntimeError, "host-owned"):
            harness.HarnessState().save()
        with self.assertRaisesRegex(ValueError, "host-owned"):
            harness.get_harness_state(state_dir="other-session")
        with self.assertRaisesRegex(ValueError, "owned by the host"):
            harness.HarnessState("other-session/harness_state.json")


if __name__ == "__main__":
    unittest.main()

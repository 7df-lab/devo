import asyncio
import copy
import unittest
from unittest.mock import AsyncMock, patch

from rlm._factory_backend import ChildSnapshot
from rlm._factory_persistence import HostFactoryStore, validate_record
from rlm._factory_spec import spec_dict, validate
from rlm.factory import FactoryNamespace
from test_factory import FakeBackend, dag


class MemoryStore:
    def __init__(self, backend):
        self.backend = backend
        self.lock = asyncio.Lock()
        self.records = {}
        self.writes = []

    async def save(self, run_id, expected_revision, snapshot):
        async with self.lock:
            return self._save(run_id, expected_revision, snapshot)

    def _save(self, run_id, expected_revision, snapshot):
        actual = self.records.get(run_id, {}).get("revision", 0)
        if actual != expected_revision:
            raise RuntimeError("stale checkpoint revision")
        record = {"run_id": run_id, "revision": actual + 1, "snapshot": copy.deepcopy(snapshot)}
        self.records[run_id] = record
        self.writes.append(copy.deepcopy(record))
        return actual + 1

    async def spawn(self, run_id, revision, node_id, prompt):
        async with self.lock:
            record = self.records[run_id]
            node = record["snapshot"]["run"]["nodes"][node_id]
            if record["revision"] != revision:
                raise RuntimeError("stale effect revision")
            if node["status"] != "admitting" or node["child_id"] is not None:
                raise RuntimeError("invalid admission")
            revision = self._save(run_id, revision, record["snapshot"])
            spec = validate(record["snapshot"]["spec"])
            child = await self.backend.spawn(next(n for n in spec.nodes if n.id == node_id), prompt, node["name"])
            return child, revision

    async def cancel(self, run_id, revision, node_id):
        async with self.lock:
            record = self.records[run_id]
            if record["revision"] != revision:
                raise RuntimeError("stale effect revision")
            node = record["snapshot"]["run"]["nodes"][node_id]
            if not record["snapshot"]["run"]["stop_requested"] or not node["needs_cleanup"]:
                raise RuntimeError("invalid cleanup")
            revision = self._save(run_id, revision, record["snapshot"])
            await self.backend.cancel(node["child_id"] or node["name"])
            return revision

    async def load(self, run_id):
        return copy.deepcopy(self.records[run_id])

    async def list(self):
        return [{"run_id": key, "revision": value["revision"], "status": value["snapshot"]["run"]["status"]}
                for key, value in self.records.items()]


class ResolvingBackend(FakeBackend):
    async def resolve(self, name):
        matches = [f"child-{i}" for i, entry in enumerate(self.spawned) if entry[2] == name]
        if len(matches) > 1:
            raise RuntimeError("ambiguous child")
        return self.children.get(matches[0]) if matches else None


class DurableFactoryTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.backend = ResolvingBackend()
        self.store = MemoryStore(self.backend)
        self.factory = FactoryNamespace(self.backend, self.store)

    async def start(self, spec=None):
        result = await self.factory.start(spec or dag("a"), enabled=True, durable=True)
        return result["id"]

    async def test_intent_is_durable_before_spawn_and_unchanged_polls_do_not_write(self):
        run_id = await self.start()
        original = self.backend.spawn

        async def spawn(node, prompt, name):
            saved = (await self.store.load(run_id))["snapshot"]["run"]["nodes"][node.id]
            self.assertEqual((saved["status"], saved["name"], saved["child_id"], saved["needs_cleanup"]),
                             ("admitting", name, None, True))
            return await original(node, prompt, name)

        with patch.object(self.backend, "spawn", spawn):
            await self.factory.advance(run_id)
        writes = len(self.store.writes)
        await self.factory.advance(run_id)
        self.assertEqual(len(self.store.writes), writes)
        self.backend.settle("child-0", "answer")
        result = await self.factory.advance(run_id)
        self.assertEqual(result["status"], "completed")
        restored = FactoryNamespace(self.backend, self.store)
        loaded = await restored.restore(run_id, enabled=True)
        self.assertEqual((loaded["status"], loaded["nodes"], len(self.backend.spawned)),
                         ("completed", result["nodes"], 1))

    async def test_restore_after_lost_spawn_ack_checkpoint_reconciles_without_replay(self):
        run_id = await self.start()
        save = self.store.save

        async def fail_after_admission(run_id, revision, snapshot):
            if snapshot["run"]["nodes"]["a"]["child_id"] is not None:
                raise RuntimeError("lost save acknowledgement")
            return await save(run_id, revision, snapshot)

        with patch.object(self.store, "save", fail_after_admission), self.assertRaisesRegex(RuntimeError, "lost"):
            await self.factory.advance(run_id)
        self.assertEqual(self.factory.status(run_id)["status"], "persistence_error")
        with self.assertRaisesRegex(RuntimeError, "restore"):
            await self.factory.advance(run_id)
        self.backend.settle("child-0", "retained")
        restored = FactoryNamespace(self.backend, self.store)
        await restored.restore(run_id, enabled=True)
        result = await restored.wait(run_id, timeout=0)
        self.assertEqual((result["status"], result["nodes"]["a"]["output"], len(self.backend.spawned)),
                         ("completed", "retained", 1))

    async def test_lost_intent_ack_never_causes_automatic_readmission(self):
        run_id = await self.start()
        save = self.store.save

        async def lost_intent_ack(run_id, revision, snapshot):
            result = await save(run_id, revision, snapshot)
            if snapshot["run"]["nodes"]["a"]["status"] == "admitting":
                raise RuntimeError("intent committed but reply lost")
            return result

        with patch.object(self.store, "save", lost_intent_ack), self.assertRaisesRegex(RuntimeError, "reply lost"):
            await self.factory.advance(run_id)
        self.assertEqual(self.backend.spawned, [])
        restored = FactoryNamespace(self.backend, self.store)
        result = await restored.restore(run_id, enabled=True)
        self.assertEqual(result["status"], "recovery_required")
        result = await asyncio.wait_for(restored.wait(run_id, timeout=300), timeout=1)
        self.assertEqual((result["status"], self.backend.spawned), ("recovery_required", []))

    async def test_save_failure_before_intent_has_no_external_side_effect(self):
        run_id = await self.start()
        with patch.object(self.store, "save", AsyncMock(side_effect=RuntimeError("denied"))):
            with self.assertRaisesRegex(RuntimeError, "denied"):
                await self.factory.advance(run_id)
        self.assertEqual(self.backend.spawned, [])
        restored = FactoryNamespace(self.backend, self.store)
        await restored.restore(run_id, enabled=True)
        await restored.advance(run_id)
        self.assertEqual(len(self.backend.spawned), 1)

    async def test_restore_claim_denial_occurs_before_child_materialization(self):
        run_id = await self.start()
        restored = FactoryNamespace(self.backend, self.store)
        with patch.object(self.store, "save", AsyncMock(side_effect=PermissionError("Plan Mode"))):
            with patch.object(self.backend, "resolve", AsyncMock()) as resolve:
                with self.assertRaises(PermissionError):
                    await restored.restore(run_id, enabled=True)
                resolve.assert_not_awaited()
        self.assertEqual(restored._runs, {})
        self.assertEqual(restored._restoring, set())

    async def test_stale_namespace_cannot_admit_after_another_restorer_claims_revision(self):
        run_id = await self.start()
        newer = FactoryNamespace(self.backend, self.store)
        await newer.restore(run_id, enabled=True)
        with self.assertRaisesRegex(RuntimeError, "stale"):
            await self.factory.advance(run_id)
        self.assertEqual(self.backend.spawned, [])
        await newer.advance(run_id)
        self.assertEqual(len(self.backend.spawned), 1)

    async def test_recovery_missing_ambiguous_or_changed_identity_never_spawns(self):
        run_id = await self.start()
        await self.factory.advance(run_id)
        for result in (None, RuntimeError("ambiguous"), ChildSnapshot("wrong-id", "running", False)):
            restored = FactoryNamespace(self.backend, self.store)
            method = AsyncMock(side_effect=result) if isinstance(result, Exception) else AsyncMock(return_value=result)
            with patch.object(self.backend, "resolve", method):
                loaded = await restored.restore(run_id, enabled=True)
            self.assertEqual(loaded["status"], "recovery_required")
            self.assertEqual((await restored.advance(run_id))["status"], "recovery_required")
        self.assertEqual(len(self.backend.spawned), 1)

    async def test_stop_failure_remains_recoverable_and_persists_terminal_cleanup(self):
        run_id = await self.start()
        await self.factory.advance(run_id)
        with patch.object(self.backend, "cancel", AsyncMock(side_effect=RuntimeError("offline"))):
            self.assertEqual((await self.factory.stop(run_id))["status"], "persistence_error")
        restored = FactoryNamespace(self.backend, self.store)
        self.assertEqual((await restored.restore(run_id, enabled=True))["status"], "stopping")
        result = await restored.stop(run_id)
        self.assertEqual(result["status"], "stopped")
        record = await self.store.load(run_id)
        _, state, _ = validate_record(record, run_id)
        self.assertEqual((state["status"], state["nodes"]["a"]["needs_cleanup"]), ("stopped", False))

    async def test_restore_and_advance_are_serialized_in_same_namespace(self):
        run_id = await self.start()
        entered, release = asyncio.Event(), asyncio.Event()
        load = self.store.load

        async def slow_load(run_id):
            entered.set()
            await release.wait()
            return await load(run_id)

        with patch.object(self.store, "load", slow_load):
            restoring = asyncio.create_task(self.factory.restore(run_id, enabled=True))
            await entered.wait()
            with self.assertRaisesRegex(RuntimeError, "being restored"):
                await self.factory.advance(run_id)
            with self.assertRaisesRegex(RuntimeError, "already being restored"):
                await self.factory.restore(run_id, enabled=True)
            release.set()
            await restoring
        self.assertEqual(self.backend.spawned, [])

    async def test_stop_during_intent_checkpoint_prevents_spawn(self):
        run_id = await self.start()
        entered, release = asyncio.Event(), asyncio.Event()
        save = self.store.save

        async def slow_intent(run_id, revision, snapshot):
            result = await save(run_id, revision, snapshot)
            if snapshot["run"]["nodes"]["a"]["status"] == "admitting":
                entered.set()
                await release.wait()
            return result

        with patch.object(self.store, "save", slow_intent):
            advancing = asyncio.create_task(self.factory.advance(run_id))
            await entered.wait()
            stopping = asyncio.create_task(self.factory.stop(run_id))
            await asyncio.sleep(0)
            release.set()
            await asyncio.gather(advancing, stopping)
        state = self.factory.status(run_id)
        self.assertEqual((state["status"], state["nodes"]["a"]["needs_cleanup"], self.backend.spawned,
                          self.backend.cancelled), ("stopped", False, [], []))
        validate_record(await self.store.load(run_id), run_id)

    async def test_interrupted_restore_is_actionable_and_never_spins_or_spawns(self):
        run_id = await self.start()
        save = self.store.save

        async def lose_intent_reply(run_id, revision, snapshot):
            result = await save(run_id, revision, snapshot)
            if snapshot["run"]["nodes"]["a"]["status"] == "admitting":
                raise RuntimeError("lost intent reply")
            return result

        with patch.object(self.store, "save", lose_intent_reply), self.assertRaises(RuntimeError):
            await self.factory.advance(run_id)
        entered = asyncio.Event()

        async def slow_resolve(name):
            entered.set()
            await asyncio.Event().wait()

        restored = FactoryNamespace(self.backend, self.store)
        with patch.object(self.backend, "resolve", slow_resolve):
            task = asyncio.create_task(restored.restore(run_id, enabled=True))
            await entered.wait()
            task.cancel()
            with self.assertRaises(asyncio.CancelledError):
                await task
        result = await asyncio.wait_for(restored.wait(run_id, timeout=300), timeout=1)
        self.assertEqual((result["status"], result["nodes"]["a"]["status"], self.backend.spawned),
                         ("recovery_required", "recovery_required", []))

    async def test_saved_is_read_only_and_restore_requires_opt_in(self):
        run_id = await self.start()
        writes = len(self.store.writes)
        self.assertEqual(await self.factory.saved(), [{"run_id": run_id, "revision": 1, "status": "ready"}])
        with self.assertRaises(ValueError):
            await self.factory.restore(run_id)
        self.assertEqual(len(self.store.writes), writes)

    async def test_recovery_validator_rejects_corruption_before_any_write(self):
        run_id = await self.start()
        original = await self.store.load(run_id)
        corruptions = [lambda r: r.update(revision=True),
                       lambda r: r["snapshot"].update(format_version=2),
                       lambda r: r["snapshot"]["run"].update(id="foreign"),
                       lambda r: r["snapshot"]["run"]["nodes"]["a"].update(name="other-child"),
                       lambda r: r["snapshot"]["run"]["nodes"]["a"].update(child_id="admitted"),
                       lambda r: r["snapshot"]["run"]["nodes"]["a"].update(status="running"),
                       lambda r: r["snapshot"]["run"].update(status="completed")]
        for change in corruptions:
            record = copy.deepcopy(original)
            change(record)
            with self.assertRaises(ValueError):
                validate_record(record, run_id)

    async def test_host_adapter_checks_acknowledgement_and_uses_scoped_routes(self):
        store = HostFactoryStore()
        import rlm
        with patch.object(rlm, "host_request", AsyncMock(return_value={"run_id": "id", "revision": 2})) as request:
            self.assertEqual(await store.save("id", 1, {}), 2)
            request.assert_awaited_once_with("factory.save", {"run_id": "id", "expected_revision": 1, "snapshot": {}})
        with patch.object(rlm, "host_request", AsyncMock(return_value={"run_id": "other", "revision": 2})):
            with self.assertRaisesRegex(RuntimeError, "acknowledgement"):
                await store.save("id", 1, {})

    async def test_completed_checkpoint_rejects_missing_outputs_duplicate_children_and_hidden_failures(self):
        run_id = await self.start(dag("a", "b"))
        await self.factory.advance(run_id)
        self.backend.settle("child-0", "A")
        self.backend.settle("child-1", "B")
        await self.factory.advance(run_id)
        original = await self.store.load(run_id)
        corruptions = [lambda r: r["snapshot"]["run"]["nodes"]["a"].update(output=None),
                       lambda r: r["snapshot"]["run"]["nodes"]["b"].update(child_id="child-0"),
                       lambda r: r["snapshot"]["run"]["nodes"]["a"].update(status="failed"),
                       lambda r: r["snapshot"]["run"].update(failed=True),
                       lambda r: r["snapshot"]["run"].update(status="stopped")]
        for change in corruptions:
            record = copy.deepcopy(original)
            change(record)
            with self.assertRaises(ValueError):
                validate_record(record, run_id)


class GuardedFactoryTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.backend = ResolvingBackend()
        self.factory = FactoryNamespace(self.backend)

    def graph(self, **options):
        return {**dag("review",
                     {"id": "accept", "prompt": "Accept", "needs": ["review"],
                      "when": {"node": "review", "path": ["approved"], "equals": True}},
                     {"id": "reject", "prompt": "Reject", "needs": ["review"],
                      "when": {"node": "review", "path": ["approved"], "equals": False}},
                     {"id": "join", "prompt": "Summarize", "needs": ["accept", "reject"], "join": "any"},
                     **options), "version": 2}

    async def test_guarded_diamond_skips_unchosen_path_and_joins_selected_result(self):
        run_id = (await self.factory.start(self.graph(), enabled=True))["id"]
        await self.factory.advance(run_id)
        self.backend.settle("child-0", '{"approved": true}')
        state = await self.factory.advance(run_id)
        self.assertEqual([n["status"] for n in state["nodes"].values()], ["completed", "running", "skipped", "pending"])
        self.backend.settle("child-1", "accepted")
        await self.factory.advance(run_id)
        self.assertEqual([x[0].id for x in self.backend.spawned], ["review", "accept", "join"])
        self.assertTrue(self.backend.spawned[2][1].endswith('{"accept": "accepted"}'))
        self.backend.settle("child-2", "summary")
        self.assertEqual((await self.factory.advance(run_id))["status"], "completed")

    async def test_all_join_propagates_skips_and_no_match_any_join_is_skipped(self):
        graph = self.graph()
        graph["nodes"][-1]["join"] = "all"
        run_id = (await self.factory.start(graph, enabled=True))["id"]
        await self.factory.advance(run_id)
        # Missing field is a non-match, never equals JSON null by accident.
        self.backend.settle("child-0", '{}')
        state = await self.factory.advance(run_id)
        self.assertEqual((state["status"], [n["status"] for n in state["nodes"].values()]),
                         ("completed", ["completed", "skipped", "skipped", "skipped"]))

    async def test_invalid_json_fails_guard_instead_of_silently_selecting_branch(self):
        run_id = (await self.factory.start(self.graph(), enabled=True))["id"]
        await self.factory.advance(run_id)
        self.backend.settle("child-0", "not JSON")
        state = await self.factory.advance(run_id)
        self.assertEqual(state["status"], "failed")
        self.assertEqual(len(self.backend.spawned), 1)
        self.assertIn("invalid guard source", state["nodes"]["accept"]["error"])

    async def test_any_join_waits_for_all_branches_and_continue_keeps_successful_output(self):
        graph = {**dag("a", "b", {"id": "join", "prompt": "Join", "needs": ["a", "b"], "join": "any"},
                      failure_policy="continue"), "version": 2}
        run_id = (await self.factory.start(graph, enabled=True))["id"]
        await self.factory.advance(run_id)
        self.backend.settle("child-0", "A")
        await self.factory.advance(run_id)
        self.assertEqual(len(self.backend.spawned), 2)
        self.backend.settle("child-1", status="failed")
        await self.factory.advance(run_id)
        self.assertEqual(len(self.backend.spawned), 3)
        self.assertTrue(self.backend.spawned[2][1].endswith('{"a": "A"}'))
        self.backend.settle("child-2", "joined")
        self.assertEqual((await self.factory.advance(run_id))["status"], "failed")

    async def test_version_two_roundtrip_and_strict_guard_validation(self):
        spec = validate(self.graph())
        self.assertEqual(validate(spec_dict(spec)), spec)
        invalid = [{"node": "missing", "equals": "yes"}, {"node": "review", "equals": True},
                   {"node": "review", "path": [1], "equals": True},
                   {"node": "review", "path": ["approved"], "equals": float("nan")},
                   {"node": "review", "path": ["approved"], "equals": 2**53},
                   {"node": "review", "path": ["approved"], "equals": -(2**53)},
                   {"node": "review", "path": [], "equals": []},
                   {"node": "review", "equals": "ok", "eval": "dangerous"}]
        for guard in invalid:
            graph = self.graph()
            graph["nodes"][1]["when"] = guard
            with self.subTest(guard=guard), self.assertRaises(ValueError):
                validate(graph)
        for value in (-(2**53 - 1), 2**53 - 1):
            graph = self.graph()
            graph["nodes"][1]["when"]["equals"] = value
            self.assertEqual(validate(spec_dict(validate(graph))), validate(graph))
        graph = self.graph()
        graph["version"] = 1
        with self.assertRaises(ValueError):
            validate(graph)


if __name__ == "__main__":
    unittest.main()

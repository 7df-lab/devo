"""Deterministic ownership/interruption regressions, without inference."""

import asyncio
import unittest
from unittest.mock import AsyncMock, patch

import rlm
from rlm._factory_persistence import HostFactoryStore
from rlm.factory import FactoryNamespace
from test_factory import dag
from test_factory_durable import MemoryStore, ResolvingBackend


class FactoryFencingTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.backend = ResolvingBackend()
        self.store = MemoryStore(self.backend)
        self.factory = FactoryNamespace(self.backend, self.store)

    async def start(self, durable=True):
        return (await self.factory.start(dag("a", "b"), enabled=True, durable=durable))["id"]

    async def test_claim_between_intent_commit_and_reply_fences_old_spawn(self):
        run_id = await self.start()
        committed, release = asyncio.Event(), asyncio.Event()
        save = self.store.save

        async def delayed_ack(run_id, revision, snapshot):
            result = await save(run_id, revision, snapshot)
            if revision == 1:
                committed.set()
                await release.wait()
            return result

        with patch.object(self.store, "save", delayed_ack):
            old = asyncio.create_task(self.factory.advance(run_id))
            await committed.wait()
            newer = FactoryNamespace(self.backend, self.store)
            state = await newer.restore(run_id, enabled=True)
            self.assertEqual(state["status"], "recovery_required")
            release.set()
            with self.assertRaisesRegex(RuntimeError, "restore"):
                await old
        self.assertEqual(self.backend.spawned, [])
        self.assertEqual(self.factory.status(run_id)["status"], "persistence_error")

    async def test_claim_between_cleanup_intent_commit_and_reply_fences_old_cancel(self):
        run_id = await self.start()
        await self.factory.advance(run_id)
        committed, release = asyncio.Event(), asyncio.Event()
        save = self.store.save
        pause = True

        async def delayed_ack(run_id, revision, snapshot):
            nonlocal pause
            result = await save(run_id, revision, snapshot)
            if pause:
                pause = False
                committed.set()
                await release.wait()
            return result

        with patch.object(self.store, "save", delayed_ack):
            old = asyncio.create_task(self.factory.stop(run_id))
            await committed.wait()
            newer = FactoryNamespace(self.backend, self.store)
            await newer.restore(run_id, enabled=True)
            release.set()
            with self.assertRaisesRegex(RuntimeError, "stale"):
                await old
        self.assertEqual(self.backend.cancelled, [])
        self.assertEqual((await newer.stop(run_id))["status"], "stopped")
        self.assertEqual(self.backend.cancelled, ["child-0", "child-1"])

    async def test_lost_effect_reply_requires_restore_and_never_duplicates_spawn(self):
        run_id = await self.start()
        spawn = self.store.spawn
        used = []

        async def lose_reply(*args):
            used.append(args)
            await spawn(*args)
            raise RuntimeError("lost effect reply")

        with patch.object(self.store, "spawn", lose_reply):
            with self.assertRaisesRegex(RuntimeError, "restore"):
                await self.factory.advance(run_id)
        with self.assertRaisesRegex(RuntimeError, "stale"):
            await spawn(*used[0])
        with self.assertRaisesRegex(RuntimeError, "restore"):
            await self.factory.advance(run_id)
        newer = FactoryNamespace(self.backend, self.store)
        await newer.restore(run_id, enabled=True)
        self.backend.settle("child-0", "retained")
        result = await newer.advance(run_id)
        self.assertEqual((result["nodes"]["a"]["output"], [n.id for n, _, _ in self.backend.spawned]),
                         ("retained", ["a", "b"]))

    async def test_lost_cancel_reply_never_repeats_same_revision(self):
        run_id = await self.start()
        await self.factory.advance(run_id)
        cancel = self.store.cancel
        used = []

        async def lose_reply(*args):
            used.append(args)
            await cancel(*args)
            raise RuntimeError("lost cancel reply")

        with patch.object(self.store, "cancel", lose_reply):
            result = await self.factory.stop(run_id)
        self.assertEqual(result["status"], "persistence_error")
        with self.assertRaisesRegex(RuntimeError, "stale"):
            await cancel(*used[0])
        self.assertEqual(self.backend.cancelled, ["child-0"])
        self.assertTrue(result["nodes"]["a"]["needs_cleanup"])

    async def test_baseexception_during_spawn_retains_cleanup_and_closes_admission(self):
        for durable in (False, True):
            for exception in (KeyboardInterrupt, asyncio.CancelledError, SystemExit):
                with self.subTest(durable=durable, exception=exception.__name__):
                    backend = ResolvingBackend()
                    factory = FactoryNamespace(backend, MemoryStore(backend))
                    run_id = (await factory.start(dag("a", "b"), enabled=True, durable=durable))["id"]
                    spawn = backend.spawn

                    async def interrupted(*args):
                        await spawn(*args)
                        raise exception()

                    with patch.object(backend, "spawn", interrupted), self.assertRaises(exception):
                        await factory.advance(run_id)
                    state = factory.status(run_id)
                    self.assertEqual((state["stop_requested"], state["nodes"]["a"]["needs_cleanup"],
                                      state["nodes"]["a"]["child_id"], state["nodes"]["b"]["status"]),
                                     (True, True, None, "pending"))
                    if durable:
                        with self.assertRaisesRegex(RuntimeError, "restore"):
                            await factory.advance(run_id)
                    else:
                        await factory.advance(run_id)
                        self.assertEqual(backend.cancelled, [state["nodes"]["a"]["name"]])
                    self.assertEqual(len(backend.spawned), 1)

    async def test_interrupted_cleanup_retains_target_and_requires_restore_if_durable(self):
        for durable in (False, True):
            for exception in (KeyboardInterrupt, asyncio.CancelledError):
                with self.subTest(durable=durable, exception=exception.__name__):
                    backend = ResolvingBackend()
                    factory = FactoryNamespace(backend, MemoryStore(backend))
                    run_id = (await factory.start(dag("a", "b"), enabled=True, durable=durable))["id"]
                    await factory.advance(run_id)
                    with patch.object(backend, "cancel", AsyncMock(side_effect=exception())), self.assertRaises(exception):
                        await factory.stop(run_id)
                    state = factory.status(run_id)
                    self.assertEqual((state["stop_requested"], state["nodes"]["a"]["child_id"],
                                      state["nodes"]["a"]["needs_cleanup"]), (True, "child-0", True))
                    if durable:
                        self.assertEqual(state["status"], "persistence_error")
                        with self.assertRaisesRegex(RuntimeError, "restore"):
                            await factory.stop(run_id)
                    else:
                        self.assertEqual((await factory.stop(run_id))["status"], "stopped")
                    self.assertEqual(len(backend.spawned), 2)

    async def test_keyboardinterrupt_during_restore_leaves_actionable_recovery_state(self):
        run_id = await self.start()
        await self.factory.advance(run_id)
        newer = FactoryNamespace(self.backend, self.store)
        with patch.object(self.backend, "resolve", AsyncMock(side_effect=KeyboardInterrupt())), self.assertRaises(KeyboardInterrupt):
            await newer.restore(run_id, enabled=True)
        self.assertEqual(newer.status(run_id)["status"], "recovery_required")
        self.assertEqual((await newer.advance(run_id))["status"], "recovery_required")
        self.assertEqual(len(self.backend.spawned), 2)

    async def test_queued_stop_rechecks_failed_effect_before_any_write_or_cancel(self):
        run_id = await self.start()
        entered, release = asyncio.Event(), asyncio.Event()

        async def uncertain_spawn(*args):
            entered.set()
            await release.wait()
            raise RuntimeError("uncertain transport failure")

        with patch.object(self.store, "spawn", uncertain_spawn):
            advancing = asyncio.create_task(self.factory.advance(run_id))
            await entered.wait()
            stopping = asyncio.create_task(self.factory.stop(run_id))
            await asyncio.sleep(0)
            writes = len(self.store.writes)
            release.set()
            with self.assertRaisesRegex(RuntimeError, "restore"):
                await advancing
            with self.assertRaisesRegex(RuntimeError, "restore"):
                await stopping
        self.assertEqual((len(self.store.writes), self.backend.cancelled), (writes, []))
        state = self.factory.status(run_id)
        self.assertEqual((state["status"], state["nodes"]["a"]["needs_cleanup"]),
                         ("persistence_error", True))

    async def test_cancelled_lock_waiter_does_not_mutate_active_scheduler(self):
        run_id = await self.start(durable=False)
        entered, release = asyncio.Event(), asyncio.Event()
        spawn = self.backend.spawn

        async def delayed(*args):
            entered.set()
            await release.wait()
            return await spawn(*args)

        with patch.object(self.backend, "spawn", delayed):
            active = asyncio.create_task(self.factory.advance(run_id))
            await entered.wait()
            waiter = asyncio.create_task(self.factory.advance(run_id))
            await asyncio.sleep(0)
            waiter.cancel()
            with self.assertRaises(asyncio.CancelledError):
                await waiter
            self.assertFalse(self.factory.status(run_id)["stop_requested"])
            release.set()
            await active
        self.assertEqual(len(self.backend.spawned), 2)

    async def test_host_effect_routes_and_invalid_acknowledgements_fail_closed(self):
        store = HostFactoryStore()
        with patch.object(rlm, "host_request", AsyncMock(return_value={"run_id": "id", "revision": 4, "child_id": "child"})) as request:
            self.assertEqual(await store.spawn("id", 3, "a", "prompt"), ("child", 4))
            request.assert_awaited_once_with("factory.spawn", {
                "run_id": "id", "expected_revision": 3, "node_id": "a", "prompt": "prompt"})
        with patch.object(rlm, "host_request", AsyncMock(return_value={"run_id": "id", "revision": 4})) as request:
            self.assertEqual(await store.cancel("id", 3, "a"), 4)
            request.assert_awaited_once_with("factory.cancel", {"run_id": "id", "expected_revision": 3, "node_id": "a"})
        for response in ({"run_id": "id", "revision": 4, "error": "close failed"},
                         {"run_id": "wrong", "revision": 4}, {"run_id": "id", "revision": True},
                         {"run_id": "id", "revision": 3}):
            with self.subTest(response=response), patch.object(rlm, "host_request", AsyncMock(return_value=response)):
                with self.assertRaisesRegex(RuntimeError, "restore"):
                    await store.cancel("id", 3, "a")

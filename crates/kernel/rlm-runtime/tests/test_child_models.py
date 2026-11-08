import unittest
from importlib import import_module
from pathlib import Path
from unittest.mock import AsyncMock, patch

import rlm

repl = import_module("rlm.repl")


class ChildModelTests(unittest.IsolatedAsyncioTestCase):
    async def test_harness_documents_discovery_and_no_fallback(self):
        harness = import_module("rlm.harness")
        overview = harness.HarnessState(in_memory=True).overview()
        for guidance in ("rlm.find_models", "thinking_levels", "model=chosen.selector",
                         "immediate parent", "without fallback"):
            self.assertIn(guidance, overview)

    async def test_spawn_wire_options_and_inheritance(self):
        response = {
            "rlm_child_id": "child", "name": "worker",
            "session_dir": "child-session", "model": "gateway/org/model",
        }
        bridge = AsyncMock(return_value={"status": "ok", "result": response})
        with patch.object(repl, "host_request", bridge):
            for options in ({}, {"model": None, "thinking": None},
                            {"model": "gateway/org/model", "thinking": "high"}):
                handle = await rlm.rlm.spawn("Review code", name="worker", **options)
                self.assertEqual(handle, rlm.RLMSpawnHandle(
                    **{**response, "session_dir": Path("child-session")}))
                bridge.assert_awaited_with({
                    "type": "rlm.run", "prompt": "Review code",
                    "kwargs": {"name": "worker", **{
                        key: value for key, value in options.items() if value is not None}},
                })

    async def test_invalid_spawn_arguments_do_not_reach_host(self):
        cases = [({"prompt": value}, TypeError) for value in (None, 1, True)]
        cases += [({"prompt": value}, ValueError) for value in ("", "  ")]
        for field in ("name", "model", "thinking"):
            cases += [({field: value}, TypeError) for value in (1, True, [])]
            cases += [({field: value}, ValueError) for value in ("", "  ")]
        cases += [({"name": None}, TypeError)]
        cases += [({"model": value}, ValueError) for value in ("model", "/id", "provider/", " /id")]
        bridge = AsyncMock()
        with patch.object(repl, "host_request", bridge):
            for overrides, error in cases:
                with self.subTest(overrides=overrides), self.assertRaises(error):
                    await rlm.spawn(**{"prompt": "Task", "name": "worker", **overrides})
        bridge.assert_not_awaited()

    async def test_find_models_wire_and_legacy_descriptors(self):
        legacy = {"provider": "gateway", "id": "org/model", "name": "Model",
                  "selector": "gateway/org/model"}
        current = {**legacy, "thinking_levels": ["off", "high"]}
        bridge = AsyncMock(return_value={"status": "ok", "result": {"models": [legacy, current]}})
        with patch.object(repl, "host_request", bridge):
            self.assertEqual(await rlm.rlm.find_models("org", 100), [
                rlm.RLMModel(**legacy), rlm.RLMModel(**current)])
            bridge.assert_awaited_with({"type": "rlm.find_models", "query": "org", "limit": 100})
            await rlm.find_models()
            bridge.assert_awaited_with({"type": "rlm.find_models", "query": "", "limit": 8})
            await rlm.find_models(limit=1)

    async def test_find_models_invalid_arguments_do_not_reach_host(self):
        bridge = AsyncMock()
        with patch.object(repl, "host_request", bridge):
            for query in (None, 1, [], True):
                with self.subTest(query=query), self.assertRaises(TypeError):
                    await rlm.find_models(query)
            for limit in (True, False, "8", 1.0, None):
                with self.subTest(limit=limit), self.assertRaises(TypeError):
                    await rlm.find_models(limit=limit)
            for limit in (0, -1, 101):
                with self.subTest(limit=limit), self.assertRaises(ValueError):
                    await rlm.find_models(limit=limit)
        bridge.assert_not_awaited()

    async def test_invalid_catalog_responses(self):
        base = {"provider": "p", "id": "m", "name": "M", "selector": "p/m"}
        payloads = [None, [], {}, {"models": None}, {"models": [None]}, {"models": [{}]}]
        payloads += [{"models": [{**base, "thinking_levels": levels}]}
                     for levels in (None, "high", [True], [""], ["  "])]
        for payload in payloads:
            with self.subTest(payload=payload), patch.object(repl, "host_request", AsyncMock(
                return_value={"status": "ok", "result": payload}
            )), self.assertRaises(RuntimeError):
                await rlm.find_models()

    async def test_host_errors_propagate_without_retry_or_fallback(self):
        for operation in ("spawn", "find_models"):
            bridge = AsyncMock(return_value={"status": "error", "error": "unsupported inherited thinking"})
            with self.subTest(operation=operation), patch.object(repl, "host_request", bridge):
                with self.assertRaisesRegex(RuntimeError, "unsupported inherited thinking"):
                    if operation == "spawn":
                        await rlm.spawn("Task", name="worker", model="gateway/org/model")
                    else:
                        await rlm.find_models()
            self.assertEqual(bridge.await_count, 1)


if __name__ == "__main__":
    unittest.main()

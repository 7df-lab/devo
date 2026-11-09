import copy
import unittest

from rlm._factory_spec import FactoryNode, FactorySpec, MAX_NODES, validate


class FactorySpecTests(unittest.TestCase):
    def test_defaults_and_topology_are_immutable(self):
        spec = {"version": 1, "nodes": [
            {"id": "review", "prompt": "Review", "needs": ["build"],
             "model": "gateway/org/model", "thinking": "high"},
            {"id": "build", "prompt": "Build"},
        ]}
        original = copy.deepcopy(spec)
        result = validate(spec)
        self.assertEqual(result, FactorySpec((
            FactoryNode("review", "Review", ("build",), "gateway/org/model", "high"),
            FactoryNode("build", "Build"),
        )))
        self.assertEqual(spec, original)
        spec["nodes"][0]["needs"].append("another")
        self.assertEqual(result.nodes[0].needs, ("build",))

    def test_invalid_specs(self):
        good = {"version": 1, "nodes": [{"id": "one", "prompt": "Task"}]}
        bad_specs = [None, [], {}, {**good, "version": True}, {**good, "version": 2},
                     {**good, "machine": {}}, {**good, "max_parallel": True},
                     {**good, "max_parallel": 0}, {**good, "max_parallel": 17},
                     {**good, "max_parallel": 1.0}, {**good, "max_parallel": None},
                     {**good, "failure_policy": "escalate"}, {**good, "nodes": []},
                     {**good, "nodes": [{}]}, {**good, "nodes": [None]},
                     {**good, "nodes": [good["nodes"][0]] * (MAX_NODES + 1)},
                     {**good, "nodes": [good["nodes"][0]] * 2}]
        node = good["nodes"][0]
        for overrides in ({"id": "Bad"}, {"id": "../other"}, {"id": "x" * 33},
                          {"prompt": " "}, {"prompt": None}, {"prompt": "x" * 128001},
                          {"needs": "one"}, {"needs": [1]}, {"needs": ["missing"]},
                          {"needs": ["one"]}, {"needs": ["one", "one"]},
                          {"model": "model"}, {"model": "/model"}, {"model": 2},
                          {"thinking": ""}, {"thinking": True}, {"retries": 1},
                          {"foreach": []}, {"guard": {}}):
            bad_specs.append({**good, "nodes": [{**node, **overrides}]})
        bad_specs.append({**good, "nodes": [
            {"id": "one", "prompt": "A", "needs": ["two"]},
            {"id": "two", "prompt": "B", "needs": ["one"]},
        ]})
        for spec in bad_specs:
            with self.subTest(spec=str(spec)[:150]), self.assertRaises(ValueError):
                validate(spec)


if __name__ == "__main__":
    unittest.main()

import unittest

from rlm.namespace_brief import namespace_brief


class NamespaceBriefTests(unittest.TestCase):
    def test_name_only_and_no_user_methods(self):
        class Hostile:
            def __repr__(self):
                raise AssertionError("repr")

            def __reduce__(self):
                raise AssertionError("pickle")

        class HostileName(str):
            def startswith(self, *_):
                raise AssertionError("custom key")

        value = Hostile()
        ns = {"task_handle": value, "x": value, "_private": value,
              "bootstrap": value, HostileName("custom"): value,
              "</live_python_names>": value, "💥": value, "a" * 65: value,
              1: value}
        before = list(ns.items())
        self.assertEqual(namespace_brief(ns, {"bootstrap"}), ["task_handle", "x"])
        self.assertEqual(list(ns.items()), before)
        self.assertIs(ns["task_handle"], value)

    def test_limits_are_applied_before_sorting_and_filtering(self):
        ns = {f"v{i}": object() for i in range(1000)}
        self.assertEqual(namespace_brief(ns, set()), sorted(f"v{i}" for i in range(32)))
        ns = {f"_private{i}": object() for i in range(256)}
        ns["late"] = object()
        self.assertEqual(namespace_brief(ns, set()), [])
        self.assertIn("late", ns)

    def test_exact_boundary_and_empty(self):
        self.assertEqual(namespace_brief({}, set()), [])
        name = "a" * 64
        self.assertEqual(namespace_brief({name: object()}, set()), [name])

"""A small read-only name sample, not object introspection or serialization."""

from itertools import islice


def namespace_brief(ns: dict, skip: set) -> list[str]:
    # Bound work before filtering or sorting. Never fetch a value: even custom
    # repr, properties, iteration and pickle hooks must stay untouched. Exact
    # builtin string keys prevent user-defined string methods from running.
    names = []
    for name in islice(ns, 256):
        if (
            type(name) is str
            and 0 < len(name) <= 64
            and name.isascii()
            and name.isidentifier()
            and not name.startswith("_")
            and name not in skip
        ):
            names.append(name)
            if len(names) == 32:
                break
    return sorted(names)

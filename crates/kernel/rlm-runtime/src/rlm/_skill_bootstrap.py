"""Execute the host's catalog selection; no discovery or configuration here.

Enablement controls automatic bindings, not Python's OS-level permissions.
Existing user references and running cells are not revoked by disabling a skill.
"""

from __future__ import annotations

import importlib.util
import inspect
import sys
import types
from pathlib import Path
from typing import Any


class _CallableSkillModule(types.ModuleType):
    async def __call__(self, *args: Any, **kwargs: Any) -> Any:
        result = self.run(*args, **kwargs)
        return await result if inspect.isawaitable(result) else result


def sync_skills(namespace: dict[str, Any], manifest: list[dict[str, str]]) -> list[str]:
    """Refresh host-managed bindings, preserving user variables and live handles."""
    previous = namespace.get("_devo_skill_bindings", {})
    selected = {entry["module"]: entry for entry in manifest}
    bindings: dict[str, Any] = {}
    for name, module in previous.items():
        entry = selected.get(name)
        if entry and Path(module.__file__).parent == Path(entry["package_dir"]):
            bindings[name] = module
            continue
        if namespace.get(name) is module:
            namespace.pop(name, None)
        if sys.modules.get(name) is module:
            # Remove only this owned package, including its relative imports.
            for key in list(sys.modules):
                if key == name or key.startswith(name + "."):
                    sys.modules.pop(key, None)
    for name, entry in selected.items():
        if name in bindings:
            continue
        package = Path(entry["package_dir"])
        original_module = sys.modules.get(name)
        try:
            spec = importlib.util.spec_from_file_location(
                name, package / "__init__.py", submodule_search_locations=[str(package)]
            )
            if spec is None or spec.loader is None:
                raise ImportError(f"Cannot load selected skill {entry['skill_path']}")
            module = importlib.util.module_from_spec(spec)
            sys.modules[name] = module
            spec.loader.exec_module(module)
            run = getattr(module, "run", None)
            if callable(run):
                module.__class__ = _CallableSkillModule
                module.__signature__ = inspect.signature(run)
                module.__doc__ = getattr(run, "__doc__", None) or module.__doc__
            bindings[name] = module
            namespace[name] = module
            namespace.pop(f"_devo_{name}_import_error", None)
        except Exception as error:
            if original_module is None:
                sys.modules.pop(name, None)
            else:
                sys.modules[name] = original_module
            namespace[f"_devo_{name}_import_error"] = str(error)
    namespace["_devo_skill_bindings"] = bindings
    return sorted(name for name, module in bindings.items() if namespace.get(name) is module)

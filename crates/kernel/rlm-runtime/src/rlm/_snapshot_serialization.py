"""Namespace serialization that never revives disk-backed file handles.

Dill reopens even closed handles in their original mode. Restoring a handle
from a completed ``with open(path, "w")`` cell would truncate the edited file.
Reject handles anywhere in an object graph and reject their legacy reducers
before they can touch the filesystem. In-memory buffers remain supported.
"""

import io

import dill


class SnapshotPickler(dill.Pickler):
    """Skip bindings containing OS file handles, including nested handles."""

    def persistent_id(self, obj):
        if isinstance(obj, io.IOBase) and not isinstance(obj, (io.BytesIO, io.StringIO)):
            raise dill.PicklingError("OS file handles cannot be restored safely")
        return None


class SnapshotUnpickler(dill.Unpickler):
    """Read current and legacy snapshots without reopening persisted files."""

    def find_class(self, module, name):
        if module in ("dill._dill", "dill.dill") and name == "_create_filehandle":
            raise dill.UnpicklingError("OS file handles cannot be restored safely")
        return super().find_class(module, name)

"""Exclusive snapshot staging without retrying Windows sandbox denials."""

import errno
import os
import uuid


def create_snapshot_temp(target: str) -> tuple[int, str]:
    # tempfile.mkstemp treats some Windows PermissionErrors as name collisions.
    # os.access does not reflect restricted-token ACLs, so that path can spin
    # through hundreds of thousands of denied opens. Only retry real collisions.
    flags = os.O_RDWR | os.O_CREAT | os.O_EXCL | getattr(os, "O_BINARY", 0)
    for _ in range(8):
        candidate = f"{target}.{uuid.uuid4().hex}.tmp"
        try:
            return os.open(candidate, flags, 0o600), candidate
        except FileExistsError:
            continue
    raise FileExistsError(errno.EEXIST, "Cannot create unique snapshot staging file", target)

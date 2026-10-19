"""Kernel-side credential grants (design doc rlm-permissions.md §9, P2).

The host delivers directory grants as file descriptors over the
``DEVO_GRANT_FD`` Unix socket: one newline-terminated JSON message per
grant, with the descriptor attached via SCM_RIGHTS and an fstat
``(dev, ino)`` anti-swap claim. Received descriptors are verified against
the claim, stored per granted root, and used by ``rlm.read``/``rlm.write``
via ``os.open(path, dir_fd=...)`` — native kernel-side I/O with no
per-operation IPC and no kernel restart.

A ``revoke`` message drops the descriptor (already-open handles keep
working — a documented residual).
"""

from __future__ import annotations

import json
import os
import socket
import struct
import threading
from typing import Any, Dict, Optional, Tuple

_grants_lock = threading.Lock()
# granted root (str, host absolute) -> (fd, access)
_grants: Dict[str, Tuple[int, str]] = {}
_started = False


def _recvmsg_json_with_fd(sock: socket.socket) -> Tuple[Dict[str, Any], int]:
    """Receive one JSON line + one SCM_RIGHTS descriptor."""
    data, ancdata, _flags, _addr = sock.recvmsg(
        65536, socket.CMSG_SPACE(struct.calcsize("i"))
    )
    fd = -1
    for level, ctype, cdata in ancdata:
        if level == socket.SOL_SOCKET and ctype == socket.SCM_RIGHTS:
            (fd,) = struct.unpack("i", cdata[: struct.calcsize("i")])
    message = json.loads(data.decode("utf-8").strip())
    return message, fd


def _accept(message: Dict[str, Any], fd: int) -> None:
    kind = message.get("type")
    root = str(message.get("root", ""))
    if kind == "revoke":
        with _grants_lock:
            entry = _grants.pop(root, None)
        if entry is not None:
            try:
                os.close(entry[0])
            except OSError:
                pass
        return
    if kind != "grant" or not root or fd < 0:
        if fd >= 0:
            try:
                os.close(fd)
            except OSError:
                pass
        return
    # Anti-swap: verify the received descriptor matches the (dev, ino) claim.
    try:
        st = os.fstat(fd)
        if st.st_dev != int(message.get("dev", -1)) or st.st_ino != int(
            message.get("ino", -1)
        ):
            os.close(fd)  # claim mismatch — drop, never trust silently
            return
    except OSError:
        return
    with _grants_lock:
        old = _grants.get(root)
        if old is not None:
            try:
                os.close(old[0])
            except OSError:
                pass
        _grants[root] = (fd, str(message.get("access", "read")))


def _reader(sock_fd: int) -> None:
    sock = socket.socket(fileno=sock_fd)
    try:
        while True:
            try:
                message, fd = _recvmsg_json_with_fd(sock)
            except (OSError, ValueError):
                return  # channel closed (kernel shutdown)
            _accept(message, fd)
    finally:
        try:
            sock.detach()
        except OSError:
            pass


def start_from_env() -> None:
    """Start the grant reader if DEVO_GRANT_FD is set (fenced kernels)."""
    global _started
    if _started:
        return
    raw = os.environ.get("DEVO_GRANT_FD")
    if not raw:
        return
    try:
        fd = int(raw)
    except ValueError:
        return
    _started = True
    thread = threading.Thread(target=_reader, args=(fd,), daemon=True)
    thread.start()


def grant_fd_for(path: os.PathLike | str) -> Optional[Tuple[int, str, str]]:
    """Longest-prefix granted root covering ``path`` -> (fd, access, rel)."""
    p = os.fspath(path)
    with _grants_lock:
        best: Optional[Tuple[int, str, str]] = None
        best_len = -1
        for root, (fd, access) in _grants.items():
            if p == root:
                rel = "."
            elif p.startswith(root.rstrip(os.sep) + os.sep):
                rel = os.path.relpath(p, root)
            else:
                continue
            if len(root) > best_len:
                best = (fd, access, rel)
                best_len = len(root)
        return best

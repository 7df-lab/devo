"""IOCP loop with a named-pipe wakeup channel for network-restricted kernels.

CPython's Windows self-pipe is a loopback TCP socket pair. Network sandboxing
can block its accept before the REPL handshake. IOCP already supports pipe
handles, so only the loop's internal wakeup channel needs replacing.
"""

import _winapi
import asyncio
import msvcrt
import os
import uuid
from asyncio import windows_utils

PIPE_NOWAIT = 1


class PipeProactorEventLoop(asyncio.ProactorEventLoop):
    """Preserve Proactor I/O while waking the loop without network access."""

    def _make_self_pipe(self) -> None:
        # asyncio.windows_utils.pipe calls tempfile.mktemp even though its name
        # lives in the named-pipe namespace. Read-only sandboxes may have no
        # writable temp directory, so create the IOCP pipe without a disk probe.
        address = rf"\\.\pipe\devo-wakeup-{os.getpid()}-{uuid.uuid4().hex}"
        read_handle = write_handle = None
        try:
            read_handle = _winapi.CreateNamedPipe(
                address,
                _winapi.PIPE_ACCESS_INBOUND | _winapi.FILE_FLAG_FIRST_PIPE_INSTANCE
                | _winapi.FILE_FLAG_OVERLAPPED,
                _winapi.PIPE_WAIT, 1, 0, windows_utils.BUFSIZE,
                _winapi.NMPWAIT_WAIT_FOREVER, _winapi.NULL,
            )
            write_handle = _winapi.CreateFile(
                address, _winapi.GENERIC_WRITE, 0, _winapi.NULL,
                _winapi.OPEN_EXISTING, 0, _winapi.NULL,
            )
            connected = _winapi.ConnectNamedPipe(read_handle, overlapped=True)
            connected.GetOverlappedResult(True)
        except BaseException:
            if read_handle is not None:
                _winapi.CloseHandle(read_handle)
            if write_handle is not None:
                _winapi.CloseHandle(write_handle)
            raise
        # PIPE_NOWAIT keeps a saturated wakeup channel from blocking a caller.
        # Use the Win32 API directly because os.set_blocking only supports
        # Windows pipes starting with Python 3.12 (the runtime supports 3.11).
        try:
            _winapi.SetNamedPipeHandleState(write_handle, PIPE_NOWAIT, None, None)
            write_fd = msvcrt.open_osfhandle(write_handle, os.O_BINARY)
        except BaseException:
            _winapi.CloseHandle(read_handle)
            _winapi.CloseHandle(write_handle)
            raise
        self._ssock = windows_utils.PipeHandle(read_handle)
        # A CRT descriptor also works with signal.set_wakeup_fd on Windows;
        # the reader retains a raw handle for overlapped IOCP reads.
        self._csock = os.fdopen(write_fd, "wb", buffering=0)
        self._internal_fds += 1

    def _write_to_self(self) -> None:
        writer = self._csock
        if writer is None:
            return
        try:
            os.write(writer.fileno(), b"\0")
        except (OSError, ValueError):
            # Saturation already guarantees a wakeup; shutdown may close the fd.
            pass

"""Byte-preserving operations for host-owned interactive bash handles."""
from __future__ import annotations

import asyncio
import base64
from dataclasses import dataclass
from typing import Any


@dataclass(frozen=True)
class BashChunk:
    """A contiguous observed segment from stdout, stderr, or a merged PTY."""

    stream: str
    data: bytes


@dataclass(frozen=True)
class BashRead:
    """One bounded cursor read. A gap is explicit; bytes are never silently decoded."""

    data: bytes
    next_cursor: int
    truncated: bool
    eof: bool
    chunks: tuple[BashChunk, ...] = ()

    @property
    def text(self) -> str:
        """Convenience UTF-8 view; use data for split UTF-8 or binary protocols."""
        return self.data.decode("utf-8", errors="replace")


def _integer(value: Any, name: str, low: int, high: int) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or not low <= value <= high:
        raise ValueError(f"{name} must be an integer between {low} and {high}")
    return value


class InteractiveBashOperations:
    """Operations shared by newly created and reattached BashHandle proxies."""

    async def ready(self):
        """Wait for host spawn, not command exit. Does not cancel the process."""
        self._released = True
        await self._spawned.wait()
        self._raise_error()
        if not self._ready:
            raise RuntimeError("bash command ended before becoming ready")
        return self

    async def _interactive_request(self, action: str, params: dict[str, Any]) -> Any:
        from . import host_request
        self._released = True
        await self._started.wait()
        if not (action == "bash.read" and self._terminal_error is not None
                and self._error is self._terminal_error):
            self._raise_error()
        if self._mode == "pipe":
            raise ValueError("this operation requires mode='interactive' or mode='pty'")
        if self._handle_id is None:
            raise RuntimeError("bash command was not admitted")
        return await host_request(action, {"handle_id": self._handle_id, **params})

    async def write(self, data: str | bytes) -> Any:
        """Write up to 64 KiB without adding a newline; never retries a failed write."""
        if isinstance(data, str):
            data = data.encode("utf-8")
        if not isinstance(data, bytes):
            raise TypeError("data must be str or bytes")
        if len(data) > 65536:
            raise ValueError("write is limited to 65536 bytes")
        await self.ready()
        return await self._interactive_request("bash.write", {
            "data": base64.b64encode(data).decode("ascii"),
        })

    async def send_line(self, text: str) -> Any:
        """Write a text line with one explicit newline."""
        if not isinstance(text, str):
            raise TypeError("text must be str")
        return await self.write(text + "\n")

    async def read(self, *, cursor: int = 0, max_bytes: int = 8192, wait_ms: int = 0) -> BashRead:
        """Read without consuming another reader's output. Reuse next_cursor explicitly."""
        _integer(cursor, "cursor", 0, 2**64 - 1)
        _integer(max_bytes, "max_bytes", 1, 65536)
        _integer(wait_ms, "wait_ms", 0, 30000)
        raw = await self._interactive_request("bash.read", {
            "cursor": cursor, "max_bytes": max_bytes, "wait_ms": wait_ms,
        })
        try:
            data = base64.b64decode(raw["data"], validate=True)
            chunks = tuple(BashChunk(item["stream"], base64.b64decode(item["data_base64"], validate=True))
                           for item in raw.get("chunks", []))
            if any(chunk.stream not in ("stdout", "stderr", "pty") for chunk in chunks):
                raise ValueError("invalid output stream")
            if chunks and b"".join(chunk.data for chunk in chunks) != data:
                raise ValueError("inconsistent output chunks")
            next_cursor = raw["next_cursor"]
            truncated, eof = raw["truncated"], raw["eof"]
            if (not isinstance(next_cursor, int) or isinstance(next_cursor, bool)
                    or next_cursor < cursor or not isinstance(truncated, bool)
                    or not isinstance(eof, bool) or len(data) > max_bytes):
                raise ValueError("invalid read metadata")
        except (KeyError, TypeError, ValueError) as exc:
            raise RuntimeError("bash host returned an invalid read") from exc
        self._note_consumed()
        return BashRead(data, next_cursor, truncated, eof, chunks)

    async def close_stdin(self) -> Any:
        """Close a pipe's input. PTY has no reliable portable half-close and is rejected."""
        await self.ready()
        if self._mode == "pty":
            raise ValueError("PTY cannot half-close stdin; send protocol input or terminate")
        return await self._interactive_request("bash.close_stdin", {})

    async def resize(self, *, cols: int, rows: int) -> Any:
        _integer(cols, "cols", 1, 1000)
        _integer(rows, "rows", 1, 1000)
        await self.ready()
        if self._mode != "pty":
            raise ValueError("resize requires mode='pty'")
        return await self._interactive_request("bash.resize", {"cols": cols, "rows": rows})

    async def wait(self, *, timeout_ms: int | None = None):
        """Wait for completion. A wait timeout does not terminate the task."""
        if timeout_ms is not None:
            _integer(timeout_ms, "timeout_ms", 0, 2**63 - 1)
        self._released = True
        if timeout_ms is None:
            return await self._wait()
        return await asyncio.wait_for(self._wait(), timeout_ms / 1000)

    async def terminate(self) -> None:
        """Request host cancellation and await acknowledgement; use wait() for exit."""
        self.kill()
        await asyncio.shield(self._kill_task)
        self._raise_error()

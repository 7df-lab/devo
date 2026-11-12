"""Host-owned shell commands with non-blocking, cached synchronous handles."""
from __future__ import annotations

import asyncio
import os
import secrets
from typing import Any

from .bash_interactive import BashChunk, BashRead, InteractiveBashOperations, _integer
from .local_bash import (
    BashResult,
    _consume_notice_task,
    _creating_cell_waits_for,
    _current_cell_completion_context,
    _live_cell_owner,
)

# Polling only refreshes cached state; synchronous access never performs RPC.
_POLL_INTERVAL = 0.1
_COMPLETION_NOTICE_COMMAND_CAP = 1000


class BashHandle(InteractiveBashOperations):
    """Awaitable host command. Creation schedules start; it does not block on approval.

    Accessors return the latest snapshot. ``handle_id`` is None until admission;
    ``pid`` is always None because a host handle is not an OS process identifier.
    ``kill()`` schedules host cancellation, and awaiting the handle waits for exit.
    """

    def __init__(self, command: str, payload: dict[str, Any], *, snapshot: dict[str, Any] | None = None) -> None:
        loop = asyncio.get_running_loop()
        self.command = command
        self._mode = payload.get("mode", "pipe")
        self._ready = False
        self._spawned = asyncio.Event()
        self._handle_id: str | None = None
        self._output = ""
        self._tail = ""
        self._result: BashResult | None = None
        self._terminal = False
        self._error: Exception | None = None
        self._terminal_error: Exception | None = None
        self._released = False
        self._consumed = False
        self._notice_accepted = False
        self._awaited_by_creating_cell = False
        self._started = asyncio.Event()
        self._admission: asyncio.Future[None] = loop.create_future()
        self._admission.add_done_callback(_consume_notice_task)
        from . import repl
        repl.register_bash_admission(self._admission)
        self._kill_task: asyncio.Task[None] | None = None
        context = _current_cell_completion_context()
        self._cell_finished = context[0] if context else None
        self._cell_task = context[1] if context else None
        self._activity = {"id": secrets.token_hex(16), "active": True}
        if context:
            from . import repl
            repl.emit({"application/vnd.bash-activity+json": self._activity})
        self._task = loop.create_task(self._run(payload, snapshot))
        self._task.add_done_callback(_consume_notice_task)
        if context:
            self._notice_task = loop.create_task(self._notify_completion())
            self._notice_task.add_done_callback(_consume_notice_task)

    @property
    def handle_id(self) -> str | None:
        self._released = True
        return self._handle_id

    @property
    def pid(self) -> None:
        self._released = True
        return None

    @property
    def running(self) -> bool:
        self._released = True
        return not self._task.done()

    def poll(self) -> BashResult | None:
        self._released = True
        self._raise_error()
        self._note_consumed()
        return self._result

    def output(self) -> str:
        self._released = True
        self._note_consumed()
        return self._output

    def tail(self, n: int = 50) -> str:
        self._released = True
        self._note_consumed()
        return "\n".join(self._tail.splitlines()[-n:]) if n > 0 else ""

    def kill(self) -> None:
        """Schedule host cancellation, including when start/approval is pending.

        Signals and grace periods belong to the host, not this opaque handle.
        """
        self._released = True
        if self._kill_task is None:
            self._kill_task = asyncio.create_task(self._kill())
            self._kill_task.add_done_callback(_consume_notice_task)

    async def _kill(self) -> None:
        from . import host_request
        await self._started.wait()
        if self._handle_id is not None and self._result is None:
            try:
                await host_request("bash.kill", {"handle_id": self._handle_id})
            except Exception as exc:
                self._error = exc
                raise

    def _raise_error(self) -> None:
        if self._error is not None:
            raise self._error

    def _apply(self, snapshot: dict[str, Any]) -> bool:
        handle_id = snapshot.get("handle_id")
        output = snapshot.get("output")
        running = snapshot.get("running")
        if not isinstance(handle_id, str) or not handle_id or not isinstance(output, str) or not isinstance(running, bool):
            raise RuntimeError("bash host returned an invalid snapshot")
        if self._handle_id is not None and handle_id != self._handle_id:
            raise RuntimeError("bash host changed the command handle")
        self._handle_id = handle_id
        mode = snapshot.get("mode", self._mode)
        if mode != self._mode:
            raise RuntimeError("bash host changed command mode")
        ready = snapshot.get("ready", self._mode == "pipe")
        if not isinstance(ready, bool):
            raise RuntimeError("bash host returned invalid readiness")
        self._ready = self._ready or ready
        if self._ready or not running:
            self._spawned.set()
        self._output = output
        tail = snapshot.get("tail")
        self._tail = tail if isinstance(tail, str) and tail else output
        self._terminal = not running
        if not running:
            code, duration = snapshot.get("exit_code"), snapshot.get("duration")
            if code is None:
                error = snapshot.get("error") or "host command ended without an exit status"
                terminal_error = RuntimeError(f"{error}\n{output}".rstrip())
                if self._mode == "pipe":
                    raise terminal_error
                # Admission of a retained record is separate from its execution
                # result: attach/read must still expose the diagnostic bytes.
                self._terminal_error = self._error = terminal_error
                return False
            if not isinstance(code, int) or not isinstance(duration, (int, float)):
                raise RuntimeError("bash host returned an invalid completion")
            self._result = BashResult(code, output, float(duration))
        return running

    async def _run(self, payload: dict[str, Any], snapshot: dict[str, Any] | None = None) -> BashResult:
        from . import host_request
        try:
            running = self._apply(snapshot if snapshot is not None else await host_request("bash.start", payload))
            self._started.set()
            self._admission.set_result(None)
            while running:
                await asyncio.sleep(_POLL_INTERVAL)
                running = self._apply(await host_request("bash.poll", {"handle_id": self._handle_id}))
            self._raise_error()
            assert self._result is not None
            return self._result
        except Exception as exc:
            self._error = exc
            if not self._admission.done():
                self._admission.set_exception(exc)
            elif self._handle_id is not None and not self._terminal and self._mode == "pipe":
                self.kill()
            raise
        finally:
            self._started.set()
            self._spawned.set()
            if self._mode == "pipe" and self._terminal and self._handle_id is not None:
                try:
                    await host_request("bash.forget", {"handle_id": self._handle_id})
                except (OSError, RuntimeError):
                    pass  # Host TTL is the backstop; cached results remain usable.

    def _note_consumed(self, awaiter: asyncio.Task[Any] | None = None) -> None:
        owner = _live_cell_owner()
        if self._result is None or self._consumed or owner is None:
            return
        if awaiter is not None and not _creating_cell_waits_for(owner, awaiter):
            return
        self._consumed = True
        if self._notice_accepted or self._mode != "pipe":
            task = asyncio.create_task(self._withdraw_notice())
            task.add_done_callback(_consume_notice_task)

    def _notice_payload(self) -> dict[str, Any]:
        command = self.command
        if len(command) > _COMPLETION_NOTICE_COMMAND_CAP:
            command = command[:_COMPLETION_NOTICE_COMMAND_CAP] + "\n... [command truncated]"
        return {"handle_id": self._handle_id, "command": command}

    async def _withdraw_notice(self) -> None:
        from . import host_request
        try:
            await host_request("bash.consumed", self._notice_payload())
        except (OSError, RuntimeError):
            pass

    async def _notify_completion(self) -> None:
        from . import host_request, repl
        try:
            result = await asyncio.shield(self._task)
            assert self._cell_finished is not None
            await self._cell_finished.wait()
            if self._mode != "pipe" or self._awaited_by_creating_cell or self._consumed or not repl.is_active():
                return
            await host_request("bash.completed", {**self._notice_payload(), "exitCode": result.exit_code})
            self._notice_accepted = True
            if self._consumed:
                await self._withdraw_notice()
        except (OSError, RuntimeError) as exc:
            import sys
            sys.stderr.write(
                f"Background bash failed for {self._handle_id!r}: {exc}. "
                "Inspect the saved handle for details.\n"
            )
        finally:
            repl.emit({"application/vnd.bash-activity+json": {
                **self._activity, "handle_id": self._handle_id, "active": False,
            }})

    async def _wait(self) -> BashResult:
        owned = not self._released
        self._released = True
        awaiter = asyncio.current_task()
        try:
            result = await asyncio.shield(self._task)
            self._note_consumed(awaiter)
            if _creating_cell_waits_for(self._cell_task, awaiter):
                self._awaited_by_creating_cell = True
            return result
        except asyncio.CancelledError:
            if owned and self._mode == "pipe":
                # Keep start alive so its eventual ID can be cancelled. Never lose
                # an admitted process by cancelling the start RPC's reply future.
                self.kill()
                if _creating_cell_waits_for(self._cell_task, awaiter):
                    self._awaited_by_creating_cell = True
            raise

    def __await__(self):
        return self._wait().__await__()

    def __repr__(self) -> str:
        state = "failed" if self._error else (
            f"exit_code={self._result.exit_code}" if self._result else
            "pending" if self._handle_id is None else "running"
        )
        return f"<BashHandle handle_id={self._handle_id!r} {state} command={self.command!r}>"


def bash(
    command: str,
    *,
    sandbox_permissions: str = "use_default",
    justification: str | None = None,
    additional_permissions: dict[str, Any] | None = None,
    prefix_rule: list[str] | None = None,
    timeout_ms: int | None = None,
    workdir: str | None = None,
    mode: str = "pipe",
    idle_timeout_ms: int | None = None,
) -> BashHandle:
    """Schedule a host shell command; no host means failure, never local fallback.

    Requires a running asyncio loop. Workdir defaults to Python's current cwd.
    Environment and shell selection are owned by the host, not ``os.environ``.
    Cancelling a first, otherwise untouched await schedules host cancellation;
    background handle awaits are wait-only. Cancellation during pending approval
    is applied after admission; it cannot dismiss a host approval dialog.
    """
    if not isinstance(command, str) or not command.strip():
        raise ValueError("command must be a non-empty string")
    if mode not in ("pipe", "interactive", "pty"):
        raise ValueError("mode must be pipe, interactive, or pty")
    if timeout_ms is not None:
        _integer(timeout_ms, "timeout_ms", 0, 2**63 - 1)
    if idle_timeout_ms is not None:
        _integer(idle_timeout_ms, "idle_timeout_ms", 1, 2**63 - 1)
        if mode == "pipe":
            raise ValueError("idle_timeout_ms requires interactive or pty mode")
    payload: dict[str, Any] = {
        "command": command,
        "workdir": os.getcwd() if workdir is None else workdir,
        "sandbox_permissions": sandbox_permissions,
    }
    if mode != "pipe":
        payload["mode"] = mode
    payload.update({key: value for key, value in {
        "justification": justification,
        "additional_permissions": additional_permissions,
        "prefix_rule": prefix_rule,
        "timeout_ms": timeout_ms,
        "idle_timeout_ms": idle_timeout_ms,
    }.items() if value is not None})
    return BashHandle(command, payload)


async def list_bash() -> list[dict[str, Any]]:
    """List retained host tasks belonging to the current agent/session."""
    from . import host_request
    result = await host_request("bash.list", {})
    handles = result.get("handles") if isinstance(result, dict) else None
    if not isinstance(handles, list) or any(not isinstance(item, dict) for item in handles):
        raise RuntimeError("bash host returned an invalid task list")
    return handles


async def attach_bash(handle_id: str) -> BashHandle:
    """Recover a proxy to a current session's interactive process, not an OS PID."""
    from . import host_request
    if not isinstance(handle_id, str) or not handle_id:
        raise ValueError("handle_id must be a non-empty string")
    snapshot = await host_request("bash.attach", {"handle_id": handle_id})
    if not isinstance(snapshot, dict) or snapshot.get("handle_id") != handle_id:
        raise RuntimeError("bash host returned an invalid task snapshot")
    command, mode = snapshot.get("command"), snapshot.get("mode")
    if not isinstance(command, str) or mode not in ("interactive", "pty"):
        raise RuntimeError("only retained interactive bash tasks can be attached")
    handle = BashHandle(command, {"mode": mode}, snapshot=snapshot)
    await asyncio.shield(handle._admission)
    return handle


bash.list = list_bash
bash.attach = attach_bash

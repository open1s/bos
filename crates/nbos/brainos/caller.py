"""Caller and Callable — RPC pattern wrappers for pybos.

Usage:
    from brainos.caller import Caller, Callable

    # Server side
    def echo(text: str) -> str:
        return f"echo:{text}"

    async with BusManager() as bus:
        srv = await Callable.create(bus.bus, "svc/echo", echo)
        await srv.start()

        # Client side
        caller = await Caller.create(bus.bus, "svc/echo")
        result = await caller.call_text("ping")  # "echo:ping"
"""

from __future__ import annotations

from typing import Any, Callable

from nbos import Caller as PyCaller
from nbos import Callable as PyCallable


class Caller:
    def __init__(self, inner: PyCaller) -> None:
        self._inner = inner

    @classmethod
    async def create(cls, bus: Any, name: str) -> Caller:
        raw = await PyCaller.create(bus, name)
        return cls(raw)

    async def call_text(self, payload: str) -> str:
        return await self._inner.call_text(payload)


class Callable:
    def __init__(self, inner: PyCallable) -> None:
        self._inner = inner

    @classmethod
    async def create(
        cls,
        bus: Any,
        uri: str,
        handler: Callable[[str], str] | None = None,
    ) -> Callable:
        raw = await PyCallable.create(bus, uri, handler)
        return cls(raw)

    async def start(self) -> None:
        await self._inner.start()

    @property
    def is_started(self) -> bool:
        return self._inner.is_started()

    async def run(self, handler: Callable[[str], str]) -> None:
        await self._inner.run(handler)

    async def run_json(self, handler: Callable[[Any], Any]) -> None:
        await self._inner.run_json(handler)

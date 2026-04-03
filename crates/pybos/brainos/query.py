"""Query and Queryable — request/response pattern wrappers for pybos.

Usage:
    from brainos.query import Query, Queryable

    # Server side
    def upper(text: str) -> str:
        return text.upper()

    async with BusManager() as bus:
        q = await Queryable.create(bus.bus, "svc/upper", upper)
        await q.start()

        # Client side
        query = await Query.create(bus.bus, "svc/upper")
        result = await query.query_text("hello")  # "HELLO"
"""

from __future__ import annotations

from typing import Any, Callable

from pybos import Query as PyQuery
from pybos import Queryable as PyQueryable


class Query:
    def __init__(self, inner: PyQuery) -> None:
        self._inner = inner

    @classmethod
    async def create(cls, bus: Any, topic: str) -> Query:
        raw = await PyQuery.create(bus, topic)
        return cls(raw)

    @property
    def topic(self) -> str:
        return self._inner.topic()

    async def query_text(self, payload: str) -> str:
        return await self._inner.query_text(payload)

    async def query_text_timeout_ms(self, payload: str, timeout_ms: int) -> str:
        return await self._inner.query_text_timeout_ms(payload, timeout_ms)


class Queryable:
    def __init__(self, inner: PyQueryable) -> None:
        self._inner = inner

    @classmethod
    async def create(
        cls,
        bus: Any,
        topic: str,
        handler: Callable[[str], str] | None = None,
    ) -> Queryable:
        raw = await PyQueryable.create(bus, topic, handler)
        return cls(raw)

    async def start(self) -> None:
        await self._inner.start()

    async def run(self, handler: Callable[[str], str]) -> None:
        await self._inner.run(handler)

    async def run_json(self, handler: Callable[[Any], Any]) -> None:
        await self._inner.run_json(handler)

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

import json
from typing import Any, Callable

from nbos_native import Query as PyQuery
from nbos_native import Queryable as PyQueryable


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

    async def ask(self, payload: str, timeout_ms: int | None = None) -> str:
        """Send a query and return the text response, with an optional timeout."""
        if timeout_ms is not None:
            return await self._inner.query_text_timeout_ms(payload, timeout_ms)
        return await self._inner.query_text(payload)

    async def ask_json(self, payload: Any, timeout_ms: int | None = None) -> Any:
        """Send ``payload`` as JSON and decode the JSON response."""
        response = await self.ask(json.dumps(payload), timeout_ms)
        try:
            return json.loads(response)
        except (TypeError, ValueError):
            return response

    async def query_text(self, payload: str) -> str:
        """Deprecated: use ``ask(payload)`` instead."""
        return await self._inner.query_text(payload)

    async def query_text_timeout_ms(self, payload: str, timeout_ms: int) -> str:
        """Deprecated: use ``ask(payload, timeout_ms)`` instead."""
        return await self._inner.query_text_timeout_ms(payload, timeout_ms)


class Queryable:
    def __init__(self, inner: PyQueryable) -> None:
        self._inner = inner
        self._handler: Callable[[str], str] | None = None

    @classmethod
    async def create(
        cls,
        bus: Any,
        topic: str,
        handler: Callable[[str], str] | None = None,
    ) -> Queryable:
        raw = await PyQueryable.create(bus, topic, handler)
        return cls(raw)

    def handle(self, handler: Callable[[str], str]) -> "Queryable":
        """Register the handler to serve once ``start`` is called."""
        self._handler = handler
        return self

    async def start(self) -> None:
        if self._handler is not None:
            await self._inner.run(self._handler)
        else:
            await self._inner.start()

    async def run(self, handler: Callable[[str], str]) -> None:
        await self._inner.run(handler)

    async def run_json(self, handler: Callable[[Any], Any]) -> None:
        await self._inner.run_json(handler)

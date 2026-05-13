"""Bus, Publisher, Subscriber — high-level async wrappers for pybos.

Usage:
    from brainos.bus import BusManager

    async with BusManager() as bus:
        await bus.publish_text("demo/topic", "hello")

        pub = await bus.create_publisher("demo/out")
        await pub.publish_text("payload")

        sub = await bus.create_subscriber("demo/in")
        msg = await sub.recv_with_timeout_ms(1000)
"""

from __future__ import annotations

from typing import Any, Callable

from pybrainos import Bus as PyBus
from pybrainos import BusConfig as PyBusConfig
from pybrainos import Publisher as PyPublisher
from pybrainos import Subscriber as PySubscriber
from pybrainos import Query as PyQuery
from pybrainos import Queryable as PyQueryable
from pybrainos import Caller as PyCaller
from pybrainos import Callable as PyCallable


# ── BusManager ──────────────────────────────────────────────────────

class BusManager:
    """Async context manager for Bus lifecycle.

    Usage:
        async with BusManager() as bus:
            await bus.publish_text("topic", "hello")
    """

    def __init__(
        self,
        *,
        mode: str = "peer",
        connect: list[str] | None = None,
        listen: list[str] | None = None,
        peer: str | None = None,
    ) -> None:
        self._mode = mode
        self._connect = connect
        self._listen = listen
        self._peer = peer
        self._bus: PyBus | None = None

    async def __aenter__(self) -> BusManager:
        cfg = PyBusConfig(
            mode=self._mode,
            connect=self._connect,
            listen=self._listen,
            peer=self._peer,
        )
        self._bus = await PyBus.create(cfg)
        return self

    async def __aexit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        pass

    # ── Convenience ────────────────────────────────────────────────

    async def publish_text(self, topic: str, payload: str) -> None:
        if self._bus is None:
            raise RuntimeError("Bus not started. Use 'async with' context.")
        await self._bus.publish_text(topic, payload)

    async def publish_json(self, topic: str, data: Any) -> None:
        if self._bus is None:
            raise RuntimeError("Bus not started. Use 'async with' context.")
        await self._bus.publish_json(topic, data)

    # ── Factory ────────────────────────────────────────────────────

    async def create_publisher(self, topic: str) -> Publisher:
        if self._bus is None:
            raise RuntimeError("Bus not started. Use 'async with' context.")
        raw = await PyPublisher.create(self._bus, topic)
        return Publisher(raw)

    async def create_subscriber(self, topic: str) -> Subscriber:
        if self._bus is None:
            raise RuntimeError("Bus not started. Use 'async with' context.")
        raw = await PySubscriber.create(self._bus, topic)
        return Subscriber(raw)

    async def create_query(self, topic: str):
        from brainos.query import Query
        if self._bus is None:
            raise RuntimeError("Bus not started. Use 'async with' context.")
        raw = await PyQuery.create(self._bus, topic)
        return Query(raw)

    async def create_queryable(self, topic: str, handler=None):
        from brainos.query import Queryable
        if self._bus is None:
            raise RuntimeError("Bus not started. Use 'async with' context.")
        raw = await PyQueryable.create(self._bus, topic, handler)
        return Queryable(raw)

    async def create_caller(self, name: str):
        from brainos.caller import Caller
        if self._bus is None:
            raise RuntimeError("Bus not started. Use 'async with' context.")
        raw = await PyCaller.create(self._bus, name)
        return Caller(raw)

    async def create_callable(self, uri: str, handler=None):
        from brainos.caller import Callable
        if self._bus is None:
            raise RuntimeError("Bus not started. Use 'async with' context.")
        raw = await PyCallable.create(self._bus, uri, handler)
        return Callable(raw)

    @property
    def bus(self) -> PyBus:
        if self._bus is None:
            raise RuntimeError("Bus not started. Use 'async with' context.")
        return self._bus


# ── Publisher wrapper ──────────────────────────────────────────────

class Publisher:
    """High-level Publisher wrapper.

    Usage:
        pub = await bus.create_publisher("my/topic")
        await pub.publish_text("hello")
        await pub.publish_json({"key": "value"})
    """

    def __init__(self, inner: PyPublisher) -> None:
        self._inner = inner

    @property
    def topic(self) -> str:
        return self._inner.topic()

    async def publish_text(self, payload: str) -> None:
        await self._inner.publish_text(payload)

    async def publish_json(self, data: Any) -> None:
        await self._inner.publish_json(data)


# ── Subscriber wrapper ─────────────────────────────────────────────

class Subscriber:
    """High-level Subscriber wrapper with async iterator support.

    Usage:
        sub = await bus.create_subscriber("my/topic")

        # One-shot receive
        msg = await sub.recv()

        # With timeout
        msg = await sub.recv_with_timeout_ms(500)

        # Async iteration
        async for msg in sub:
            print(msg)

        # Callback loop
        await sub.run(lambda m: print(m))
    """

    def __init__(self, inner: PySubscriber) -> None:
        self._inner = inner

    async def recv(self) -> str | None:
        return await self._inner.recv()

    async def recv_with_timeout_ms(self, timeout_ms: int) -> str | None:
        return await self._inner.recv_with_timeout_ms(timeout_ms)

    async def recv_json_with_timeout_ms(self, timeout_ms: int) -> Any | None:
        return await self._inner.recv_json_with_timeout_ms(timeout_ms)

    async def run(self, callback: Callable[[str], None]) -> None:
        await self._inner.run(callback)

    async def run_json(self, callback: Callable[[Any], None]) -> None:
        await self._inner.run_json(callback)

    # ── Async iterator ─────────────────────────────────────────────

    def __aiter__(self) -> Subscriber:
        return self

    async def __anext__(self) -> str:
        msg = await self._inner.recv()
        if msg is None:
            raise StopAsyncIteration
        return msg

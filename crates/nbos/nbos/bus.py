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

from nbos_native import Bus as PyBus
from nbos_native import BusConfig as PyBusConfig
from nbos_native import Publisher as PyPublisher
from nbos_native import Subscriber as PySubscriber
from nbos_native import Query as PyQuery
from nbos_native import Queryable as PyQueryable
from nbos_native import Caller as PyCaller
from nbos_native import Callable as PyCallable


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

    @classmethod
    async def create(
        cls,
        *,
        mode: str = "peer",
        connect: list[str] | None = None,
        listen: list[str] | None = None,
        peer: str | None = None,
    ) -> "BusManager":
        """Construct and start a manager in one call."""
        return await cls(mode=mode, connect=connect, listen=listen, peer=peer).start()

    async def start(self) -> "BusManager":
        """Create the underlying bus if it does not exist yet."""
        if self._bus is None:
            cfg = PyBusConfig(
                mode=self._mode,
                connect=self._connect,
                listen=self._listen,
                peer=self._peer,
            )
            self._bus = await PyBus.create(cfg)
        return self

    async def stop(self) -> None:
        """Close the underlying bus and drop the handle."""
        if self._bus is not None:
            await self._bus.close()
            self._bus = None

    # ── Fluent configuration ───────────────────────────────────────

    def mode(self, mode: str) -> "BusManager":
        self._mode = mode
        return self

    def connect(self, addresses: list[str]) -> "BusManager":
        self._connect = addresses
        return self

    def listen(self, addresses: list[str]) -> "BusManager":
        self._listen = addresses
        return self

    def peer(self, peer: str) -> "BusManager":
        self._peer = peer
        return self

    async def __aenter__(self) -> BusManager:
        return await self.start()

    async def __aexit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        await self.stop()

    # ── Convenience ────────────────────────────────────────────────

    async def publish_text(self, topic: str, payload: str) -> None:
        if self._bus is None:
            raise RuntimeError("Bus not started. Use 'async with' context.")
        await self._bus.publish_text(topic, payload)

    async def publish_json(self, topic: str, data: Any) -> None:
        if self._bus is None:
            raise RuntimeError("Bus not started. Use 'async with' context.")
        await self._bus.publish_json(topic, data)

    async def publish(self, topic: str, payload: Any, is_json: bool = False) -> None:
        """Publish ``payload`` to ``topic``, JSON-encoding it when asked."""
        if is_json:
            await self.publish_json(topic, payload)
        else:
            await self.publish_text(topic, payload)

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
        from nbos.query import Query
        if self._bus is None:
            raise RuntimeError("Bus not started. Use 'async with' context.")
        raw = await PyQuery.create(self._bus, topic)
        return Query(raw)

    async def create_queryable(self, topic: str, handler=None):
        from nbos.query import Queryable
        if self._bus is None:
            raise RuntimeError("Bus not started. Use 'async with' context.")
        raw = await PyQueryable.create(self._bus, topic, handler)
        return Queryable(raw)

    async def create_caller(self, name: str):
        from nbos.caller import Caller
        if self._bus is None:
            raise RuntimeError("Bus not started. Use 'async with' context.")
        raw = await PyCaller.create(self._bus, name)
        return Caller(raw)

    async def create_callable(self, uri: str, handler=None):
        from nbos.caller import Callable
        if self._bus is None:
            raise RuntimeError("Bus not started. Use 'async with' context.")
        raw = await PyCallable.create(self._bus, uri, handler)
        return Callable(raw)

    # ── Fluent factories ───────────────────────────────────────────

    async def publisher(self, topic: str) -> Publisher:
        """Fluent alias for :meth:`create_publisher`."""
        return await self.create_publisher(topic)

    async def subscriber(self, topic: str) -> Subscriber:
        """Fluent alias for :meth:`create_subscriber`."""
        return await self.create_subscriber(topic)

    async def query(self, topic: str):
        """Fluent alias for :meth:`create_query`."""
        return await self.create_query(topic)

    async def queryable(self, topic: str, handler=None):
        """Fluent alias for :meth:`create_queryable`."""
        return await self.create_queryable(topic, handler)

    async def caller(self, name: str):
        """Fluent alias for :meth:`create_caller`."""
        return await self.create_caller(name)

    async def callable(self, name: str, handler=None):
        """Fluent alias for :meth:`create_callable`."""
        return await self.create_callable(name, handler)

    @property
    def bus(self) -> PyBus:
        if self._bus is None:
            raise RuntimeError("Bus not started. Use 'async with' context.")
        return self._bus

    @property
    def session_id(self) -> str:
        """Return the identifier of the underlying session."""
        return self.bus.session_id()


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

    async def publish(self, payload: Any, is_json: bool = False) -> None:
        """Publish ``payload``, JSON-encoding it when ``is_json`` is true."""
        if is_json:
            await self.publish_json(payload)
        else:
            await self.publish_text(payload)

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
        msg = await sub.recv(500)

        # Async iteration
        async for msg in sub:
            print(msg)

        # Callback loop
        await sub.run(lambda m: print(m))
    """

    def __init__(self, inner: PySubscriber) -> None:
        self._inner = inner

    @property
    def topic(self) -> str:
        return self._inner.topic()

    async def recv(self, timeout_ms: int | None = None) -> str | None:
        """Receive the next message, waiting at most ``timeout_ms`` if given."""
        if timeout_ms is not None:
            return await self._inner.recv_with_timeout_ms(timeout_ms)
        return await self._inner.recv()

    async def recv_json(self, timeout_ms: int | None = None) -> Any | None:
        """Receive and decode the next JSON message, with an optional timeout."""
        if timeout_ms is not None:
            return await self._inner.recv_json_with_timeout_ms(timeout_ms)
        return await self._inner.recv_json()

    async def recv_with_timeout_ms(self, timeout_ms: int) -> str | None:
        """Deprecated: use ``recv(timeout_ms)`` instead."""
        return await self.recv(timeout_ms)

    async def recv_json_with_timeout_ms(self, timeout_ms: int) -> Any | None:
        """Deprecated: use ``recv_json(timeout_ms)`` instead."""
        return await self.recv_json(timeout_ms)

    async def run(self, callback: Callable[[str], None]) -> None:
        await self._inner.run(callback)

    async def run_json(self, callback: Callable[[Any], None]) -> None:
        await self._inner.run_json(callback)

    async def next(self) -> dict[str, Any]:
        """One async-iterator step: ``{"done": bool, "value": message}``."""
        message = await self.recv()
        if message is None:
            return {"done": True, "value": None}
        return {"done": False, "value": message}

    # ── Async iterator ─────────────────────────────────────────────

    def __aiter__(self) -> Subscriber:
        return self

    async def __anext__(self) -> str:
        step = await self.next()
        if step["done"]:
            raise StopAsyncIteration
        return step["value"]

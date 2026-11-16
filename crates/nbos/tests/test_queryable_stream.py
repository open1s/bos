"""Tests for Queryable stream_handler (server-side streaming)"""
import pytest
import asyncio
from nbos import Bus, BusConfig, Query, Queryable, StreamSender


class TestQueryableStream:
    """Queryable run_stream tests"""

    @pytest.mark.asyncio
    async def test_stream_yields_multiple_chunks(self):
        """Test that a stream handler can yield multiple replies per query"""
        def echo_stream_handler(text: str, sender: StreamSender):
            words = text.split()
            for word in words:
                sender.send(f"word:{word}")

        bus = await Bus.create(BusConfig())
        queryable = await Queryable.create(bus, "test/stream")
        await queryable.run_stream(echo_stream_handler)

        await asyncio.sleep(0.1)

        query = await Query.create(bus, "test/stream")
        stream_iter = await query.stream_text("hello world from stream")

        collected = []
        async for chunk in stream_iter:
            collected.append(chunk)

        assert len(collected) == 4
        assert "word:hello" in collected
        assert "word:world" in collected
        assert "word:from" in collected
        assert "word:stream" in collected

    @pytest.mark.asyncio
    async def test_stream_single_chunk(self):
        """Test that stream handler works with a single send"""
        def single_handler(text: str, sender: StreamSender):
            sender.send(f"echo:{text}")

        bus = await Bus.create(BusConfig())
        queryable = await Queryable.create(bus, "test/single")
        await queryable.run_stream(single_handler)

        await asyncio.sleep(0.1)

        query = await Query.create(bus, "test/single")
        stream_iter = await query.stream_text("ping")

        collected = []
        async for chunk in stream_iter:
            collected.append(chunk)

        assert len(collected) == 1
        assert collected[0] == "echo:ping"

    @pytest.mark.asyncio
    async def test_stream_progressive(self):
        """Test that chunks arrive progressively (not batched)"""
        def progressive_handler(text: str, sender: StreamSender):
            for i in range(3):
                sender.send(f"step:{i}")

        bus = await Bus.create(BusConfig())
        queryable = await Queryable.create(bus, "test/progressive")
        await queryable.run_stream(progressive_handler)

        await asyncio.sleep(0.1)

        query = await Query.create(bus, "test/progressive")
        stream_iter = await query.stream_text("go")

        collected = []
        async for chunk in stream_iter:
            collected.append(chunk)

        assert len(collected) == 3
        assert collected[0] == "step:0"
        assert collected[1] == "step:1"
        assert collected[2] == "step:2"

    @pytest.mark.asyncio
    async def test_stream_overflow_reports_the_buffer_limit(self):
        """A handler that overfills the reply buffer must not deadlock.

        The bus buffers 32 replies and drains them only after the handler
        returns, so blocking on capacity (the old behaviour) parked the Tokio
        worker while holding the GIL and hung the whole interpreter.
        """
        overflow_errors: list[str] = []

        def flooding_handler(text: str, sender: StreamSender):
            for i in range(40):
                try:
                    sender.send(f"step:{i}")
                except RuntimeError as exc:
                    overflow_errors.append(str(exc))
                    return

        bus = await Bus.create(BusConfig())
        queryable = await Queryable.create(bus, "test/stream-overflow")
        await queryable.run_stream(flooding_handler)

        await asyncio.sleep(0.1)

        query = await Query.create(bus, "test/stream-overflow")
        stream_iter = await query.stream_text("go")

        async def drain() -> list[str]:
            return [chunk async for chunk in stream_iter]

        collected = await asyncio.wait_for(drain(), timeout=10)

        assert len(overflow_errors) == 1, overflow_errors
        assert "stream reply buffer is full" in overflow_errors[0]
        assert 0 < len(collected) <= 32

    @pytest.mark.asyncio
    async def test_stream_handler_exception_reaches_the_caller(self):
        """An exception raised by the handler must surface, not truncate silently."""
        def failing_handler(text: str, sender: StreamSender):
            raise ValueError("handler exploded")

        bus = await Bus.create(BusConfig())
        queryable = await Queryable.create(bus, "test/stream-failing")
        await queryable.run_stream(failing_handler)

        await asyncio.sleep(0.1)

        query = await Query.create(bus, "test/stream-failing")
        stream_iter = await query.stream_text("boom")

        async def drain() -> list[str]:
            return [chunk async for chunk in stream_iter]

        # The failure must reach the caller instead of ending the stream with
        # no replies. The bus reports it as a generic reply error, so only the
        # exception type is asserted here.
        with pytest.raises(RuntimeError):
            await asyncio.wait_for(drain(), timeout=10)

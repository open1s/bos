"""Tests for Query stream_text streaming"""
import pytest
import asyncio
from pybrainos import Bus, BusConfig, Query, Queryable


class TestQueryStream:
    """Query stream_text streaming tests"""

    @pytest.mark.asyncio
    async def test_stream_returns_async_iterator(self):
        """Test that stream_text() returns an async iterator"""
        bus = await Bus.create(BusConfig())
        query = await Query.create(bus, "test/stream")
        
        stream_iter = await query.stream_text("hello")
        assert stream_iter is not None
        assert hasattr(stream_iter, '__anext__')
        assert hasattr(stream_iter, '__aiter__')

    @pytest.mark.asyncio
    async def test_stream_yields_results(self):
        """Test that stream_text yields results from queryable"""
        def multi_handler(text: str) -> str:
            return f"echo:{text}"

        bus = await Bus.create(BusConfig())
        queryable = await Queryable.create(bus, "test/echo", multi_handler)
        await queryable.start()

        await asyncio.sleep(0.1)

        query = await Query.create(bus, "test/echo")
        stream_iter = await query.stream_text("hello")
        
        collected = []
        async for chunk in stream_iter:
            collected.append(chunk)
        
        # Should get at least one result
        assert len(collected) >= 1
        assert "echo:hello" in collected

    @pytest.mark.asyncio
    async def test_stream_multiple_results(self):
        """Test streaming with multiple queryable handlers on same topic"""
        def handler_a(text: str) -> str:
            return f"A:{text}"

        def handler_b(text: str) -> str:
            return f"B:{text}"

        bus = await Bus.create(BusConfig())
        
        qa = await Queryable.create(bus, "test/multi", handler_a)
        await qa.start()
        
        qb = await Queryable.create(bus, "test/multi", handler_b)
        await qb.start()

        await asyncio.sleep(0.1)

        query = await Query.create(bus, "test/multi")
        stream_iter = await query.stream_text("ping")
        
        collected = []
        async for chunk in stream_iter:
            collected.append(chunk)
        
        # Should get results from both handlers
        assert len(collected) >= 2
        assert any("A:ping" in c for c in collected)
        assert any("B:ping" in c for c in collected)

    @pytest.mark.asyncio
    async def test_stream_exhaustion(self):
        """Test that stream iterator properly exhausts"""
        def handler(text: str) -> str:
            return f"done:{text}"

        bus = await Bus.create(BusConfig())
        queryable = await Queryable.create(bus, "test/exhaust", handler)
        await queryable.start()

        await asyncio.sleep(0.1)

        query = await Query.create(bus, "test/exhaust")
        stream_iter = await query.stream_text("test")
        
        # Iterate manually
        results = []
        try:
            while True:
                try:
                    chunk = await stream_iter.__anext__()
                    results.append(chunk)
                except StopAsyncIteration:
                    break
        except Exception:
            pass
        
        # Should have completed without hanging
        assert len(results) >= 1

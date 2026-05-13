"""Tests for Queryable stream_handler (server-side streaming)"""
import pytest
import asyncio
from pybrainos import Bus, BusConfig, Query, Queryable, StreamSender


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

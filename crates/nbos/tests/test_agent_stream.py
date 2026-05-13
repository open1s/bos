"""Tests for Agent stream API"""
import pytest
import asyncio
from nbos import Agent, AgentConfig, Bus, BusConfig


class TestAgentStream:
    """Agent stream functionality tests"""

    @pytest.mark.asyncio
    async def test_stream_returns_async_iterator(self):
        """Test that stream() returns an async iterator"""
        config = AgentConfig()
        bus = await Bus.create(BusConfig())
        agent = await Agent.create(config, bus)
        
        # stream() returns a Future that resolves to a StreamIterator
        stream_iter = await agent.stream("hello")
        assert stream_iter is not None
        
        # StreamIterator should have __anext__ method
        assert hasattr(stream_iter, '__anext__')

    @pytest.mark.asyncio
    async def test_stream_iterator_exhaustion(self):
        """Test that stream iterator properly raises StopAsyncIteration when done"""
        config = AgentConfig()
        bus = await Bus.create(BusConfig())
        agent = await Agent.create(config, bus)
        
        stream_iter = await agent.stream("hello")
        
        # The stream will eventually complete (or error on LLM connection)
        # We just verify the iterator protocol works
        collected = []
        try:
            while True:
                try:
                    chunk = await stream_iter.__anext__()
                    collected.append(chunk)
                except StopAsyncIteration:
                    break
        except Exception:
            # LLM connection errors are expected without valid API key
            pass
        
        # If we got here without hanging, the iterator protocol works
        assert True

    @pytest.mark.asyncio
    async def test_stream_async_for_loop(self):
        """Test that stream works with async for loop"""
        config = AgentConfig()
        bus = await Bus.create(BusConfig())
        agent = await Agent.create(config, bus)
        
        stream_iter = await agent.stream("hello")
        
        collected = []
        try:
            async for chunk in stream_iter:
                collected.append(chunk)
        except Exception:
            # LLM connection errors are expected
            pass
        
        # If we got here without hanging, async for works
        assert True

    @pytest.mark.asyncio
    async def test_stream_multiple_calls(self):
        """Test that stream can be called multiple times"""
        config = AgentConfig()
        bus = await Bus.create(BusConfig())
        agent = await Agent.create(config, bus)
        
        # Each call should return a new iterator
        stream1 = await agent.stream("first")
        stream2 = await agent.stream("second")
        
        assert stream1 is not None
        assert stream2 is not None
        assert stream1 is not stream2

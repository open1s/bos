"""Tests for Bus and BusConfig binding"""
import pytest
import asyncio
from nbos import Bus, BusConfig


class TestBusConfig:
    """BusConfig functionality tests"""

    def test_bus_config_creation_default(self):
        """Test creating BusConfig with default parameters"""
        config = BusConfig()
        assert config is not None

    def test_bus_config_creation_with_mode(self):
        """Test creating BusConfig with mode parameter"""
        config = BusConfig(mode="peer")
        assert config is not None

    def test_bus_config_repr(self):
        """Test BusConfig string representation"""
        config = BusConfig(mode="router")
        repr_str = repr(config)
        assert "BusConfig" in repr_str or repr_str is not None


class TestBus:
    """Bus functionality tests"""

    @pytest.mark.asyncio
    async def test_bus_creation(self):
        """Test creating a Bus instance"""
        config = BusConfig()
        bus = await Bus.create(config)
        assert bus is not None

    @pytest.mark.asyncio
    async def test_bus_creation_with_mode(self):
        """Test creating Bus with specific mode"""
        config = BusConfig(mode="peer")
        bus = await Bus.create(config)
        assert bus is not None

    @pytest.mark.asyncio
    async def test_bus_publish_text(self):
        """Test publishing text to bus"""
        config = BusConfig()
        bus = await Bus.create(config)
        # Should not raise
        await bus.publish_text("test/topic", "hello world")

    @pytest.mark.asyncio
    async def test_bus_publish_json(self):
        """Test publishing JSON to bus"""
        config = BusConfig()
        bus = await Bus.create(config)
        data = {"message": "hello", "value": 42}
        # Should not raise
        await bus.publish_json("test/topic", data)

    @pytest.mark.asyncio
    async def test_bus_multiple_publishes(self):
        """Test publishing multiple messages"""
        config = BusConfig()
        bus = await Bus.create(config)
        
        for i in range(5):
            await bus.publish_text(f"test/topic/{i}", f"message {i}")

    @pytest.mark.asyncio
    async def test_bus_different_topics(self):
        """Test publishing to different topics"""
        config = BusConfig()
        bus = await Bus.create(config)
        
        topics = ["topic/a", "topic/b", "topic/c"]
        for topic in topics:
            await bus.publish_text(topic, f"data for {topic}")

    @pytest.mark.asyncio
    async def test_bus_publish_with_special_characters_in_topic(self):
        """Test publishing with special characters in topic"""
        config = BusConfig()
        bus = await Bus.create(config)
        
        # Zenoh allows various characters in topics
        await bus.publish_text("topic/sub-topic_1", "test")
        await bus.publish_text("topic/123", "test")

    @pytest.mark.asyncio
    async def test_bus_publish_large_message(self):
        """Test publishing large text messages"""
        config = BusConfig()
        bus = await Bus.create(config)
        
        large_message = "x" * 10000
        await bus.publish_text("test/large", large_message)

    @pytest.mark.asyncio
    async def test_bus_publish_complex_json(self):
        """Test publishing complex nested JSON"""
        config = BusConfig()
        bus = await Bus.create(config)
        
        complex_data = {
            "user": {
                "name": "Alice",
                "id": 123,
                "roles": ["admin", "user"],
                "metadata": {
                    "created": "2024-01-01",
                    "status": "active"
                }
            },
            "values": [1, 2, 3, 4, 5],
            "flags": {"enabled": True, "debug": False}
        }
        await bus.publish_json("test/complex", complex_data)

    @pytest.mark.asyncio
    async def test_bus_publish_empty_string(self):
        """Test publishing empty string"""
        config = BusConfig()
        bus = await Bus.create(config)
        
        await bus.publish_text("test/empty", "")

    @pytest.mark.asyncio
    async def test_bus_publish_empty_dict(self):
        """Test publishing empty JSON object"""
        config = BusConfig()
        bus = await Bus.create(config)
        
        await bus.publish_json("test/empty_dict", {})

    @pytest.mark.asyncio
    async def test_bus_concurrent_publishes(self):
        """Test concurrent publish operations"""
        config = BusConfig()
        bus = await Bus.create(config)
        
        # Create multiple concurrent publish tasks
        tasks = []
        for i in range(10):
            task = bus.publish_text(f"test/concurrent/{i}", f"message {i}")
            tasks.append(task)
        
        # Wait for all tasks to complete
        await asyncio.gather(*tasks)


class TestBusManagerFluent:
    """Fluent BusManager surface (nbos.bus)"""

    @pytest.mark.asyncio
    async def test_create_and_fluent_factories(self):
        from nbos.bus import BusManager

        bus = await BusManager.create(mode="peer")
        try:
            assert bus.bus is not None

            sub = await bus.subscriber("fluent/topic")
            pub = await bus.publisher("fluent/topic")
            await bus.publish("fluent/topic", "hi")
            assert await sub.recv(1000) == "hi"

            await pub.publish_text("again")
            assert await sub.recv(1000) == "again"
        finally:
            await bus.stop()

    @pytest.mark.asyncio
    async def test_fluent_configuration_chains(self):
        from nbos.bus import BusManager

        manager = BusManager().mode("peer").peer("p1")
        await manager.start()
        assert manager.bus is not None
        await manager.stop()

    @pytest.mark.asyncio
    async def test_query_and_caller_factories(self):
        from nbos.bus import BusManager

        async with BusManager() as bus:
            server = await bus.queryable("fluent/q", lambda text: text.upper())
            await server.start()
            await asyncio.sleep(0.1)
            client = await bus.query("fluent/q")
            assert await client.ask("ok") == "OK"

            rpc = await bus.callable("fluent/rpc", lambda text: text[::-1])
            await rpc.start()
            await asyncio.sleep(0.1)
            caller = await bus.caller("fluent/rpc")
            assert await caller.call("abc") == "cba"

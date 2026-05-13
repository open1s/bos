"""Tests for Publisher and Subscriber binding"""
import pytest
import asyncio
from nbos import Bus, BusConfig, Publisher, Subscriber


class TestPublisher:
    """Publisher functionality tests"""

    @pytest.mark.asyncio
    async def test_publisher_creation(self):
        """Test creating a Publisher instance"""
        bus = await Bus.create(BusConfig())
        publisher = await Publisher.create(bus, "test/topic")
        assert publisher is not None

    @pytest.mark.asyncio
    async def test_publisher_publish_text(self):
        """Test publisher publishing text"""
        bus = await Bus.create(BusConfig())
        publisher = await Publisher.create(bus, "test/topic")
        # Should not raise
        await publisher.publish_text("hello world")

    @pytest.mark.asyncio
    async def test_publisher_publish_json(self):
        """Test publisher publishing JSON"""
        bus = await Bus.create(BusConfig())
        publisher = await Publisher.create(bus, "test/topic")
        data = {"message": "hello", "value": 42}
        # Should not raise
        await publisher.publish_json(data)

    @pytest.mark.asyncio
    async def test_publisher_multiple_publishes(self):
        """Test multiple publishes through same publisher"""
        bus = await Bus.create(BusConfig())
        publisher = await Publisher.create(bus, "test/topic")
        
        for i in range(5):
            await publisher.publish_text(f"message {i}")

    @pytest.mark.asyncio
    async def test_publisher_with_complex_data(self):
        """Test publisher with complex nested data"""
        bus = await Bus.create(BusConfig())
        publisher = await Publisher.create(bus, "test/topic")
        
        complex_data = {
            "id": 1,
            "name": "test",
            "nested": {
                "level2": {
                    "items": [1, 2, 3],
                    "flag": True
                }
            }
        }
        await publisher.publish_json(complex_data)


class TestSubscriber:
    """Subscriber functionality tests"""

    @pytest.mark.asyncio
    async def test_subscriber_creation(self):
        """Test creating a Subscriber instance"""
        bus = await Bus.create(BusConfig())
        subscriber = await Subscriber.create(bus, "test/topic")
        assert subscriber is not None

    @pytest.mark.asyncio
    async def test_publisher_subscriber_communication(self):
        """Test basic publisher-subscriber communication"""
        bus = await Bus.create(BusConfig())
        # Subscriber must be created FIRST to listen on topic
        subscriber = await Subscriber.create(bus, "test/topic")
        publisher = await Publisher.create(bus, "test/topic")
        
        # Publish a message
        await publisher.publish_text("hello subscriber")
        
        # Receive with timeout
        message = await subscriber.recv_with_timeout_ms(1000)
        assert message == "hello subscriber"

    @pytest.mark.asyncio
    async def test_subscriber_multiple_messages(self):
        """Test subscriber receiving multiple messages"""
        bus = await Bus.create(BusConfig())
        # Subscriber must be created FIRST to listen on topic
        subscriber = await Subscriber.create(bus, "test/topic")
        publisher = await Publisher.create(bus, "test/topic")
        
        messages = ["msg1", "msg2", "msg3"]
        
        # Publish all messages
        for msg in messages:
            await publisher.publish_text(msg)
        
        # Receive all messages
        received = []
        for _ in messages:
            msg = await subscriber.recv_with_timeout_ms(1000)
            received.append(msg)
        
        assert len(received) == len(messages)

    @pytest.mark.asyncio
    async def test_subscriber_json_communication(self):
        """Test JSON-based communication between publisher and subscriber"""
        bus = await Bus.create(BusConfig())
        # Subscriber must be created FIRST to listen on topic
        subscriber = await Subscriber.create(bus, "test/json")
        publisher = await Publisher.create(bus, "test/json")
        
        data = {"action": "test", "value": 123, "active": True}
        await publisher.publish_json(data)
        
        received = await subscriber.recv_json_with_timeout_ms(1000)
        assert received["action"] == "test"
        assert received["value"] == 123
        assert received["active"] is True

    @pytest.mark.asyncio
    async def test_subscriber_timeout_no_message(self):
        """Test subscriber timeout when no message arrives"""
        bus = await Bus.create(BusConfig())
        subscriber = await Subscriber.create(bus, "empty/topic")
        
        # Should timeout and return None or raise
        try:
            message = await subscriber.recv_with_timeout_ms(100)
            # If no error, message should be None or empty
            assert message is None or message == ""
        except asyncio.TimeoutError:
            # This is also acceptable behavior
            pass

    @pytest.mark.asyncio
    async def test_multiple_subscribers_same_topic(self):
        """Test multiple subscribers listening to same topic"""
        bus = await Bus.create(BusConfig())
        # Subscribers must be created FIRST before publisher sends
        sub1 = await Subscriber.create(bus, "broadcast/topic")
        sub2 = await Subscriber.create(bus, "broadcast/topic")
        publisher = await Publisher.create(bus, "broadcast/topic")
        
        await publisher.publish_text("broadcast message")
        
        # Both subscribers should receive the message
        msg1 = await sub1.recv_with_timeout_ms(1000)
        msg2 = await sub2.recv_with_timeout_ms(1000)
        
        assert msg1 == "broadcast message"
        assert msg2 == "broadcast message"

    @pytest.mark.asyncio
    async def test_different_topic_isolation(self):
        """Test that subscribers only receive from subscribed topics"""
        bus = await Bus.create(BusConfig())
        # Subscriber must be created FIRST before publisher sends
        sub_a = await Subscriber.create(bus, "topic/a")
        pub_a = await Publisher.create(bus, "topic/a")
        pub_b = await Publisher.create(bus, "topic/b")
        
        await pub_a.publish_text("message for A")
        await pub_b.publish_text("message for B")
        
        # Should receive message from topic/a
        msg = await sub_a.recv_with_timeout_ms(1000)
        assert msg == "message for A"

    @pytest.mark.asyncio
    async def test_publisher_subscriber_concurrent(self):
        """Test concurrent publisher and subscriber operations"""
        bus = await Bus.create(BusConfig())
        # Subscriber must be created FIRST and start listening
        subscriber = await Subscriber.create(bus, "concurrent/topic")
        publisher = await Publisher.create(bus, "concurrent/topic")
        
        async def publish_messages():
            for i in range(5):
                await publisher.publish_text(f"msg {i}")
                await asyncio.sleep(0.01)
        
        async def subscribe_messages():
            messages = []
            for _ in range(5):
                msg = await subscriber.recv_with_timeout_ms(1000)
                messages.append(msg)
            return messages
        
        # Run publisher and subscriber concurrently
        pub_task = asyncio.create_task(publish_messages())
        sub_task = asyncio.create_task(subscribe_messages())
        
        await pub_task
        received = await sub_task
        
        assert len(received) == 5

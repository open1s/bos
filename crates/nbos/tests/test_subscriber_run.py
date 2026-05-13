#!/usr/bin/env python3
"""
Test Subscriber.run() with Python callback
"""
import asyncio
import sys
sys.path.insert(0, '/Users/gaosg/Projects/bos/crates/pybos')

from nbos import Bus, BusConfig, Publisher, Subscriber


async def test_subscriber_run_text():
    """Test Subscriber.run with text callback"""
    print("\n📝 Testing Subscriber.run() with text messages...")
    
    bus = await Bus.create(BusConfig())
    
    # Create subscriber first (listening)
    subscriber = await Subscriber.create(bus, "test/run/text")
    
    # Create publisher
    publisher = await Publisher.create(bus, "test/run/text")
    
    # Collect messages with callback
    messages = []
    def on_message(msg):
        print(f"  ✓ Received: {msg}")
        messages.append(msg)
    
    # Start subscriber in background with callback
    # Run in parallel with publisher using gather
    async def run_subscriber():
        try:
            await subscriber.run(on_message)
        except asyncio.CancelledError:
            pass
    
    # Start the subscriber task
    run_task = asyncio.create_task(run_subscriber())
    
    # Give subscriber time to start listening
    await asyncio.sleep(0.2)
    
    # Publish some messages
    print("  Publishing messages...")
    await publisher.publish_text("hello")
    await publisher.publish_text("world")
    
    # Give time for messages to be processed
    await asyncio.sleep(0.3)
    
    # Cancel the run task
    run_task.cancel()
    try:
        await run_task
    except asyncio.CancelledError:
        pass
    
    print(f"  Received {len(messages)} messages: {messages}")
    assert len(messages) == 2, f"Expected 2 messages, got {len(messages)}"
    assert messages[0] == "hello", f"Expected 'hello', got '{messages[0]}'"
    assert messages[1] == "world", f"Expected 'world', got '{messages[1]}'"
    print("  ✅ Text message test passed!")


async def test_subscriber_run_json():
    """Test Subscriber.run_json with JSON callback"""
    print("\n📦 Testing Subscriber.run_json() with JSON messages...")
    
    bus = await Bus.create(BusConfig())
    
    # Create subscriber first (listening)
    subscriber = await Subscriber.create(bus, "test/run/json")
    
    # Create publisher
    publisher = await Publisher.create(bus, "test/run/json")
    
    # Collect messages with callback
    messages = []
    def on_message(data):
        print(f"  ✓ Received: {data}")
        messages.append(data)
    
    # Start subscriber in background with JSON callback
    async def run_subscriber():
        try:
            await subscriber.run_json(on_message)
        except asyncio.CancelledError:
            pass
    
    # Start the subscriber task
    run_task = asyncio.create_task(run_subscriber())
    
    # Give subscriber time to start listening
    await asyncio.sleep(0.2)
    
    # Publish JSON messages
    print("  Publishing JSON messages...")
    await publisher.publish_json({"action": "start", "id": 1})
    await publisher.publish_json({"action": "stop", "id": 2})
    
    # Give time for messages to be processed
    await asyncio.sleep(0.3)
    
    # Cancel the run task
    run_task.cancel()
    try:
        await run_task
    except asyncio.CancelledError:
        pass
    
    print(f"  Received {len(messages)} messages")
    assert len(messages) == 2, f"Expected 2 messages, got {len(messages)}"
    assert messages[0]["action"] == "start", f"Expected 'start', got '{messages[0]['action']}'"
    assert messages[1]["action"] == "stop", f"Expected 'stop', got '{messages[1]['action']}'"
    print("  ✅ JSON message test passed!")


async def main():
    print("=" * 60)
    print("Testing Subscriber.run() with Python callbacks")
    print("=" * 60)
    
    try:
        await test_subscriber_run_text()
        await test_subscriber_run_json()
        print("\n" + "=" * 60)
        print("✅ All tests passed!")
        print("=" * 60)
    except Exception as e:
        print(f"\n❌ Test failed: {e}")
        import traceback
        traceback.print_exc()
        return 1
    
    return 0


if __name__ == "__main__":
    exit_code = asyncio.run(main())
    sys.exit(exit_code)

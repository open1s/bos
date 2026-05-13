#!/usr/bin/env python3
"""
BusManager Demo — Pub/Sub with async context manager

Demonstrates:
1. BusManager lifecycle with async context manager
2. Direct publish_text / publish_json
3. Publisher / Subscriber creation and usage
4. Subscriber as async iterator
5. Subscriber with callback loop
"""

import asyncio
import json

from nbos.bus import BusManager


async def demo_direct_publish():
    print("═" * 60)
    print("  Demo 1 — Direct publish via BusManager")
    print("═" * 60)

    async with BusManager() as bus:
        sub = await bus.create_subscriber("demo/greet")

        async def recv_loop():
            msg = await sub.recv_with_timeout_ms(2000)
            if msg:
                print(f"  📨 Received: {msg}")

        task = asyncio.create_task(recv_loop())
        await asyncio.sleep(0.1)

        await bus.publish_text("demo/greet", "Hello from BusManager!")
        await task

        print("  ✅ Direct publish done\n")


async def demo_publisher_subscriber():
    print("═" * 60)
    print("  Demo 2 — Publisher & Subscriber objects")
    print("═" * 60)

    async with BusManager() as bus:
        pub = await bus.create_publisher("demo/events")
        sub = await bus.create_subscriber("demo/events")

        async def recv_loop():
            msg = await sub.recv_with_timeout_ms(2000)
            if msg:
                print(f"  📨 Subscriber received: {msg}")

        task = asyncio.create_task(recv_loop())
        await asyncio.sleep(0.1)

        await pub.publish_text("event-fired")
        await task

        await pub.publish_json({"action": "deploy", "service": "api", "version": "2.0"})
        json_msg = await sub.recv_json_with_timeout_ms(2000)
        if json_msg:
            print(f"  📨 JSON received: service={json_msg.get('service')}, version={json_msg.get('version')}")

        print("  ✅ Publisher/Subscriber done\n")


async def demo_async_iterator():
    print("═" * 60)
    print("  Demo 3 — Subscriber as async iterator")
    print("═" * 60)

    async with BusManager() as bus:
        pub = await bus.create_publisher("demo/stream")
        sub = await bus.create_subscriber("demo/stream")

        async def publish_batch():
            for i in range(3):
                await pub.publish_text(f"message-{i}")
                await asyncio.sleep(0.05)

        pub_task = asyncio.create_task(publish_batch())

        count = 0
        async for msg in sub:
            print(f"  📨 Iterator received: {msg}")
            count += 1
            if count >= 3:
                break

        await pub_task
        print("  ✅ Async iterator done\n")


async def demo_callback_loop():
    print("═" * 60)
    print("  Demo 4 — Subscriber callback loop")
    print("═" * 60)

    async with BusManager() as bus:
        pub = await bus.create_publisher("demo/callback")
        sub = await bus.create_subscriber("demo/callback")

        received = []

        def on_message(msg):
            received.append(msg)
            print(f"  📨 Callback received: {msg}")

        async def run_sub():
            try:
                await sub.run(on_message)
            except asyncio.CancelledError:
                pass

        sub_task = asyncio.create_task(run_sub())
        await asyncio.sleep(0.1)

        for i in range(3):
            await pub.publish_text(f"callback-msg-{i}")
            await asyncio.sleep(0.05)

        sub_task.cancel()
        try:
            await sub_task
        except asyncio.CancelledError:
            pass

        print(f"  ✅ Callback loop done — received {len(received)} messages\n")


async def main():
    print("\n" + "🚌" * 30)
    print("  BrainOS — BusManager Demo")
    print("🚌" * 30 + "\n")

    await demo_direct_publish()
    await demo_publisher_subscriber()
    await demo_async_iterator()
    await demo_callback_loop()

    print("═" * 60)
    print("  ✅ All BusManager demos completed!")
    print("═" * 60 + "\n")


if __name__ == "__main__":
    asyncio.run(main())

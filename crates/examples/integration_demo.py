#!/usr/bin/env python3
"""
Full Integration Demo — All brainos components working together

Demonstrates:
1. Config loading and discovery
2. Bus lifecycle with BusManager
3. Pub/Sub event streaming
4. Query/Queryable request-response
5. Caller/Callable RPC
6. BrainOS agent with tools
"""

import asyncio
import json

from nbos.bus import BusManager
from nbos.config import Config
from nbos.query import Query, Queryable
from nbos.caller import Caller, Callable


async def demo_config_and_bus():
    print("═" * 60)
    print("  Step 1 — Config + Bus lifecycle")
    print("═" * 60)

    cfg = Config()
    cfg.discover()
    data = cfg.load_sync()
    print(f"  ⚙️  Config keys: {list(data.keys()) if data else '(empty)'}")

    async with BusManager() as bus:
        print(f"  🚌 Bus started (mode=peer)")
        
        sub = await bus.create_subscriber("system/ready")
        
        await bus.publish_text("system/ready", "all components online")
        print(f"  📨 Published: system/ready = 'all components online'")

        msg = await sub.recv_with_timeout_ms(1000)
        print(f"  📨 Received: '{msg}'")

    print("  ✅ Config + Bus done\n")


async def demo_pubsub():
    print("═" * 60)
    print("  Step 2 — Pub/Sub event streaming")
    print("═" * 60)

    async with BusManager() as bus:
        pub = await bus.create_publisher("events/user-action")
        sub = await bus.create_subscriber("events/user-action")

        async def recv_one():
            return await sub.recv_json_with_timeout_ms(2000)

        task = asyncio.create_task(recv_one())
        await asyncio.sleep(0.1)

        await pub.publish_json({
            "action": "login",
            "user": "alice",
            "timestamp": "2026-04-03T10:00:00Z",
        })

        result = await task
        if result:
            print(f"  📨 Event received: user={result.get('user')}, action={result.get('action')}")

    print("  ✅ Pub/Sub done\n")


async def demo_query():
    print("═" * 60)
    print("  Step 3 — Query/Queryable request-response")
    print("═" * 60)

    async with BusManager() as bus:
        def word_count(text: str) -> str:
            words = len(text.split())
            return json.dumps({"text_length": len(text), "word_count": words})

        q = await Queryable.create(bus.bus, "svc/wordcount", word_count)
        await q.start()

        query = await Query.create(bus.bus, "svc/wordcount")
        resp = await query.query_text("hello world from nbos")
        result = json.loads(resp)
        print(f"  📤 Query: 'hello world from nbos'")
        print(f"  📥 Response: words={result['word_count']}, chars={result['text_length']}")

    print("  ✅ Query done\n")


async def demo_rpc():
    print("═" * 60)
    print("  Step 4 — Caller/Callable RPC")
    print("═" * 60)

    async with BusManager() as bus:
        def reverse_handler(text: str) -> str:
            return text[::-1]

        srv = await Callable.create(bus.bus, "rpc/reverse", reverse_handler)
        await srv.start()

        caller = await Caller.create(bus.bus, "rpc/reverse")
        result = await caller.call_text("brainos")
        print(f"  📤 Call: 'brainos'")
        print(f"  📥 Response: '{result}'")

    print("  ✅ RPC done\n")


async def main():
    print("\n" + "🧠" * 30)
    print("  BrainOS — Full Integration Demo")
    print("🧠" * 30 + "\n")

    await demo_config_and_bus()
    await demo_pubsub()
    await demo_query()
    await demo_rpc()

    print("═" * 60)
    print("  ✅ All integration demos completed!")
    print("═" * 60 + "\n")


if __name__ == "__main__":
    asyncio.run(main())

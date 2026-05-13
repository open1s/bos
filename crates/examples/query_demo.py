#!/usr/bin/env python3
"""
Query / Queryable Demo — Request/Response pattern

Demonstrates:
1. Queryable server with inline handler
2. Query client sending requests
3. Request/response with timeout
"""

import asyncio
import json

from nbos.bus import BusManager
from nbos.query import Query, Queryable


async def demo_inline_handler():
    print("═" * 60)
    print("  Demo 1 — Queryable with inline handler")
    print("═" * 60)

    async with BusManager() as bus:
        def upper_handler(text: str) -> str:
            return text.upper()

        q = await Queryable.create(bus.bus, "svc/upper", upper_handler)
        await q.start()

        query = await Query.create(bus.bus, "svc/upper")
        result = await query.query_text("hello world")
        print(f"  📤 Query: 'hello world'")
        print(f"  📥 Response: '{result}'")
        print(f"  ✅ Inline handler done\n")


async def demo_run_handler():
    print("═" * 60)
    print("  Demo 2 — Queryable with run() handler")
    print("═" * 60)

    async with BusManager() as bus:
        def echo_handler(request: str) -> str:
            data = json.loads(request)
            data["echoed"] = True
            return json.dumps(data)

        q = await Queryable.create(bus.bus, "svc/echo")

        async def run_q():
            try:
                await q.run(echo_handler)
            except asyncio.CancelledError:
                pass

        task = asyncio.create_task(run_q())
        await asyncio.sleep(0.2)

        query = await Query.create(bus.bus, "svc/echo")
        resp = await query.query_text(json.dumps({"msg": "ping"}))
        result = json.loads(resp)
        print(f"  📤 Query: {result}")
        print(f"  ✅ Run handler done\n")

        task.cancel()
        try:
            await task
        except asyncio.CancelledError:
            pass


async def demo_timeout():
    print("═" * 60)
    print("  Demo 3 — Query with timeout")
    print("═" * 60)

    async with BusManager() as bus:
        def slow_handler(text: str) -> str:
            return f"processed: {text}"

        q = await Queryable.create(bus.bus, "svc/slow", slow_handler)
        await q.start()

        query = await Query.create(bus.bus, "svc/slow")
        result = await query.query_text_timeout_ms("test-data", 5000)
        print(f"  📤 Query with 5s timeout: 'test-data'")
        print(f"  📥 Response: '{result}'")
        print(f"  ✅ Timeout query done\n")


async def main():
    print("\n" + "🔍" * 30)
    print("  BrainOS — Query / Queryable Demo")
    print("🔍" * 30 + "\n")

    await demo_inline_handler()
    await demo_run_handler()
    await demo_timeout()

    print("═" * 60)
    print("  ✅ All Query demos completed!")
    print("═" * 60 + "\n")


if __name__ == "__main__":
    asyncio.run(main())

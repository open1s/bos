#!/usr/bin/env python3
"""
Caller / Callable Demo — RPC pattern

Demonstrates:
1. Callable server with inline handler
2. Caller making RPC calls
3. JSON request/response
"""

import asyncio
import json

from brainos.bus import BusManager
from brainos.caller import Caller, Callable


async def demo_inline_handler():
    print("═" * 60)
    print("  Demo 1 — Callable with inline handler")
    print("═" * 60)

    async with BusManager() as bus:
        def add_handler(req: str) -> str:
            a, b = map(int, req.split(","))
            return str(a + b)

        srv = await Callable.create(bus.bus, "rpc/add", add_handler)
        await srv.start()

        caller = await Caller.create(bus.bus, "rpc/add")
        result = await caller.call_text("5,7")
        print(f"  📤 Call: '5,7'")
        print(f"  📥 Response: '{result}'")
        print(f"  ✅ Inline handler done\n")


async def demo_run_handler():
    print("═" * 60)
    print("  Demo 2 — Callable with run() handler")
    print("═" * 60)

    async with BusManager() as bus:
        def multiply(req: str) -> str:
            a, b = map(int, req.split(","))
            return str(a * b)

        srv = await Callable.create(bus.bus, "rpc/mul")

        async def run_srv():
            try:
                await srv.run(multiply)
            except asyncio.CancelledError:
                pass

        task = asyncio.create_task(run_srv())
        await asyncio.sleep(0.2)

        caller = await Caller.create(bus.bus, "rpc/mul")
        result = await caller.call_text("6,7")
        print(f"  📤 Call: '6,7'")
        print(f"  📥 Response: '{result}'")
        print(f"  ✅ Run handler done\n")

        task.cancel()
        try:
            await task
        except asyncio.CancelledError:
            pass


async def demo_json_rpc():
    print("═" * 60)
    print("  Demo 3 — Callable with JSON handler")
    print("═" * 60)

    async with BusManager() as bus:
        def greet_handler(data):
            name = data.get("name", "World")
            return {"greeting": f"Hello, {name}!", "status": "ok"}

        srv = await Callable.create(bus.bus, "rpc/greet")

        async def run_srv():
            try:
                await srv.run_json(greet_handler)
            except asyncio.CancelledError:
                pass

        task = asyncio.create_task(run_srv())
        await asyncio.sleep(0.2)

        caller = await Caller.create(bus.bus, "rpc/greet")
        result = await caller.call_text(json.dumps({"name": "Alice"}))
        resp = json.loads(result)
        print(f"  📤 Call: {{\"name\": \"Alice\"}}")
        print(f"  📥 Response: {resp}")
        print(f"  ✅ JSON RPC done\n")

        task.cancel()
        try:
            await task
        except asyncio.CancelledError:
            pass


async def main():
    print("\n" + "📞" * 30)
    print("  BrainOS — Caller / Callable Demo")
    print("📞" * 30 + "\n")

    await demo_inline_handler()
    await demo_run_handler()
    await demo_json_rpc()

    print("═" * 60)
    print("  ✅ All Caller/Callable demos completed!")
    print("═" * 60 + "\n")


if __name__ == "__main__":
    asyncio.run(main())

#!/usr/bin/env python3
"""Test async tools, hooks, and plugins in nbos."""

import sys, asyncio, json
from nbos import BrainOS, ToolDef

async def async_weather_callback(args):
    city = args.get("city", "Unknown")
    await asyncio.sleep(0.05)
    return json.dumps({"city": city, "temp": 68, "unit": "F"})

def create_async_tool():
    return ToolDef(
        name="get_weather_async",
        description="Fetch weather from async API",
        callback=async_weather_callback,
        parameters={"type": "object", "properties": {"city": {"type": "string"}}}
    )

async def main():
    print("=" * 50, flush=True)
    print("  nbos Async Tools Test", flush=True)
    print("=" * 50, flush=True)

    async with BrainOS() as brain:
        builder = brain.agent("test").register(create_async_tool())
        agent = await builder.start()
        print(f"\nTools: {[t.name for t in agent._tools.list_tools()]}", flush=True)

        print("\nTest: Weather in Tokyo (async tool)", flush=True)
        result = await agent.react("What's the weather in Tokyo? Use get_weather_async.")
        print(f"Result: {result}", flush=True)

    print("\nDone!", flush=True)

asyncio.run(main())
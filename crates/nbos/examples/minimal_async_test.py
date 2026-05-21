#!/usr/bin/env python3
"""Minimal test for async tool."""

import sys
print("Step 1: Starting", flush=True)

import asyncio
import json
from nbos import BrainOS, ToolDef

print("Step 7: nbos imported", flush=True)

async def async_weather_callback(args):
    print(f"  [async_weather] Called!", flush=True)
    return json.dumps({"city": "test", "temp": 68})

def create_async_tool():
    return ToolDef(
        name="get_weather_async",
        description="Async weather",
        callback=async_weather_callback,
        parameters={"type": "object", "properties": {"city": {"type": "string"}}}
    )

async def main():
    print("Step 8: main() started", flush=True)
    brain = BrainOS()
    print("Step 9: BrainOS created", flush=True)
    await brain.__aenter__()
    print("Step 10: Context entered", flush=True)
    
    print("Step 11: Creating tool...", flush=True)
    tool = create_async_tool()
    print(f"Step 11.5: Tool created: {tool.name}", flush=True)
    
    print("Step 12: Creating builder...", flush=True)
    builder = brain.agent("test").register(tool)
    print("Step 13: Builder created", flush=True)
    
    print("Step 14: Starting agent...", flush=True)
    agent = await builder.start()
    print("Step 15: Agent started", flush=True)
    
    print("Step 16: Running simple question...", flush=True)
    result = await agent.run_simple("Say hello")
    print(f"Result: {result[:50]}...", flush=True)
    
    await brain.__aexit__(None, None, None)
    print("Step 17: Done!", flush=True)

print("Step 18: Calling asyncio.run()", flush=True)
asyncio.run(main())
print("Step 19: asyncio.run() completed", flush=True)

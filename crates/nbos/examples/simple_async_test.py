#!/usr/bin/env python3
"""Simple test for async tool."""

import asyncio
import json
from nbos import BrainOS, tool, ToolDef

async def async_weather_callback(args):
    city = args.get("city", "Unknown")
    print(f"  [async_weather] Called with city={city}")
    await asyncio.sleep(0.1)
    return json.dumps({"city": city, "temp": 68, "unit": "F"})

def create_async_tool():
    return ToolDef(
        name="get_weather_async",
        description="Fetch weather data from async API",
        callback=async_weather_callback,
        parameters={
            "type": "object",
            "properties": {
                "city": {"type": "string", "description": "City name"}
            },
            "required": ["city"]
        }
    )

async def main():
    print("Testing async tool...")
    
    async with BrainOS() as brain:
        builder = (
            brain.agent("test")
            .register(create_async_tool())
        )
        
        agent = await builder.start()
        print(f"Tools: {[t.name for t in agent._tools.list_tools()]}")
        
        # Test with run_simple (no tools)
        print("\nTest 1: Simple question")
        result = await agent.run_simple("Say hello")
        print(f"Result: {result[:50]}...")
        
        # Test with react (uses tools)
        print("\nTest 2: Tool call")
        result = await agent.react("What's the weather in Tokyo? Use get_weather_async.")
        print(f"Result: {result[:100]}...")

if __name__ == "__main__":
    asyncio.run(main())

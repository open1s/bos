#!/usr/bin/env python3
"""Test async tools, hooks, and plugins in nbos."""

import asyncio
import json
from nbos import BrainOS, tool, ToolDef

# ─── Async Tools ─────────────────────────────────────────────────────────────

@tool("Fetch weather data from API", name="get_weather")
def get_weather_sync(args):
    """Sync tool for comparison."""
    city = args.get("city", "Unknown")
    return json.dumps({"city": city, "temp": 72, "unit": "F"})

def create_async_weather_tool():
    """Create an async tool using ToolDef with async callback."""
    async def async_weather_callback(args):
        city = args.get("city", "Unknown")
        # Simulate async API call
        await asyncio.sleep(0.1)
        return json.dumps({"city": city, "temp": 68, "unit": "F", "source": "async_api"})

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

# ─── Async Hooks ─────────────────────────────────────────────────────────────

async def async_before_llm_hook(event, ctx):
    """Async hook that simulates rate limit check."""
    await asyncio.sleep(0.05)
    print(f"  ⏳ [Async Hook:{event}] Rate limit check passed")
    return "Continue"

async def async_after_llm_hook(event, ctx):
    """Async hook that simulates logging."""
    await asyncio.sleep(0.05)
    print(f"  ⏳ [Async Hook:{event}] Logged response")
    return "Continue"

# ─── Async Plugins ───────────────────────────────────────────────────────────

async def async_on_llm_request(request):
    """Async plugin that enriches request."""
    await asyncio.sleep(0.05)
    print(f"  ⏳ [Async Plugin:on_llm_request] Enriching request for model={request.model}")
    request.temperature = 0.7
    return request

async def async_on_llm_response(response):
    """Async plugin that analyzes response."""
    await asyncio.sleep(0.05)
    print(f"  ⏳ [Async Plugin:on_llm_response] Analyzing response type={response.response_type}")
    return response

# ─── Main Test ───────────────────────────────────────────────────────────────

async def main():
    print("=" * 60)
    print("  BrainOS — Async Tools, Hooks & Plugins Test")
    print("=" * 60)

    async with BrainOS() as brain:
        # Build agent with async tools, hooks, and plugins
        builder = (
            brain.agent("async-test")
            .with_tools(get_weather_sync)
            .register(create_async_weather_tool())
            .with_hooks({
                "BeforeLlmCall": async_before_llm_hook,
                "AfterLlmCall": async_after_llm_hook,
            })
        )

        # Register async plugin
        plugin = {
            "name": "AsyncEnricher",
            "on_llm_request": async_on_llm_request,
            "on_llm_response": async_on_llm_response,
        }
        builder.with_plugins(plugin)

        agent = await builder.start()

        print("\n" + "─" * 60)
        print("  Registered tools:")
        print(f"    {[t.name for t in agent._tools.list_tools()]}")

        print("\n" + "─" * 60)
        print("  Test 1 — Simple question (async hooks + plugin)")
        print("─" * 60)

        result = await agent.run_simple("What is 2+2?")
        print(f"  Result: {result[:100]}...")

        print("\n" + "─" * 60)
        print("  Test 2 — Tool call (sync tool)")
        print("─" * 60)

        result = await agent.react("What's the weather in Paris? Use the get_weather tool.")
        print(f"  Result: {result[:200]}...")

        print("\n" + "─" * 60)
        print("  Test 3 — Tool call (async tool)")
        print("─" * 60)

        result = await agent.react("What's the weather in Tokyo? Use the get_weather_async tool.")
        print(f"  Result: {result[:200]}...")

    print("\n" + "=" * 60)
    print("  ✅ All async tests completed!")
    print("=" * 60)

if __name__ == "__main__":
    asyncio.run(main())

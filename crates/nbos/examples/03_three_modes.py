#!/usr/bin/env python3
"""
Example 3: Three Ways to Talk to an Agent

Demonstrates:
- agent.run_simple() — Single LLM call with tools and skills
- agent.stream()   — Streaming token-by-token response
"""

import asyncio
import json
from datetime import datetime, timezone
from pybrainos import ConfigLoader as PyConfigLoader

from brainos import BrainOS, tool


@tool("Get current UTC time")
def current_time() -> dict:
    return {"utc_time": datetime.now(timezone.utc).isoformat()}


@tool("Get weather for a city")
def weather(city: str) -> dict:
    return {"city": city, "temperature": 22, "condition": "sunny"}


async def main():
    async with BrainOS() as brain:
        loader = PyConfigLoader()
        loader.discover()
        config = loader.load_sync()

        global_model = config.get("global_model", {})
        model = global_model.get("model")

        agent = await (
            brain.agent("multi-mode", system_prompt="You are a helpful assistant.")
            .with_tools(current_time, weather)
            .start()
        )

        # ── Mode 1: react() — Direct conversation, no tools ──
        print("=== Mode 1: agent.react() — Direct conversation ===")
        for q in ["What time is it?", "Weather in Paris?"]:
            print(f"  Q: {q}")
            result = await agent.react(q)
            print(f"  A: {result}\n")

        # ── Mode 2: run_simple() — Knowledge / computation ──
        print("=== Mode 2: agent.run_simple() — Knowledge & computation ===")
        for msg in ["Tell me a fun fact about Paris.", "What is the capital of Japan?"]:
            print(f"  You: {msg}")
            reply = await agent.run_simple(msg)
            print(f"  Agent: {reply}\n")

        # ── Mode 3: stream() — Token-by-token streaming ──
        print("=== Mode 3: agent.stream() — Streaming response ===")
        for prompt in ["Describe the sky.", "What is your favorite animal?"]:
            print(f"  You: {prompt}")
            print("  Agent: ", end="", flush=True)
            try:
                stream_iter = await agent.stream(prompt)
                async for chunk in stream_iter:
                    print(chunk, end="", flush=True)
            except RuntimeError as e:
                print(f"[stream error: {e}]", end="", flush=True)
            print("\n")


if __name__ == "__main__":
    asyncio.run(main())

#!/usr/bin/env python3
"""
Example 3: Conversational Agent — Multi-turn chat with context.

Demonstrates:
- agent.chat() for natural conversation (no ReAct)
- agent.ask() for tool-aware reasoning
- Switching between conversation and tool use
"""

import asyncio
import json
from datetime import datetime, timezone
from pybos import ConfigLoader as PyConfigLoader

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
        agent = (
            brain.agent("chatbot", system_prompt="You are a friendly conversational assistant.")
            .register_many(current_time, weather)
        )

        # Natural conversation (no tool use)
        print("=== Conversation ===")
        for msg in ["Hello!", "What can you help me with?"]:
            print(f"  You: {msg}")
            reply = await agent.chat(msg)
            print(f"  Agent: {reply}\n")

        # Tool-aware queries
        print("=== Tool Queries ===")
        for q in ["What time is it?", "Weather in Paris?"]:
            print(f"  Q: {q}")
            result = await agent.ask(q)
            print(f"  A: {result}\n")


if __name__ == "__main__":
    asyncio.run(main())

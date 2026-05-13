#!/usr/bin/env python3
"""
Example 2: Multi-Tool Agent — Weather, Time, Calculator.

Demonstrates:
- Multiple tool types with different parameter signatures
- Agent autonomously choosing which tool to call
- Mixed queries requiring multiple tool calls
"""

import asyncio
import json
from datetime import datetime, timezone
from pybrainos import ConfigLoader as PyConfigLoader

from brainos import BrainOS, tool


@tool("Evaluate a mathematical expression")
def calculator(expression: str) -> dict:
    allowed = set("0123456789+-*/.() ")
    if not all(c in allowed for c in expression):
        return {"error": "Invalid expression"}
    try:
        return {"expression": expression, "result": eval(expression, {"__builtins__": {}}, {})}
    except Exception as e:
        return {"error": str(e)}


@tool("Get current weather for a city")
def weather(city: str) -> dict:
    return {
        "city": city,
        "temperature": 22,
        "unit": "°C",
        "condition": "sunny",
        "humidity": 45,
    }


@tool("Get current UTC time")
def current_time() -> dict:
    return {"utc_time": datetime.now(timezone.utc).isoformat(), "timezone": "UTC"}


async def main():
    async with BrainOS() as brain:
        loader = PyConfigLoader()
        loader.discover()
        config = loader.load_sync()

        global_model = config.get("global_model", {})
        model = global_model.get("model")
        agent = await (
            brain.agent("assistant", system_prompt="You are a helpful assistant. Use tools when available.")
            .with_tools(calculator, weather, current_time)
            .start()
        )

        print(f"Registered tools: {agent.tools}\n")

        queries = [
            "What is 1234 * 5678?",
            "What's the weather in Tokyo?",
            "What time is it now?",
        ]

        for q in queries:
            print(f"Q: {q}")
            result = await agent.ask(q)
            print(f"A: {result}\n")


if __name__ == "__main__":
    asyncio.run(main())

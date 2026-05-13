#!/usr/bin/env python3
"""
Example 1: Quick Start — Minimal BrainOS agent with tools.

Demonstrates:
- @tool decorator for tool creation
- BrainOS context manager
- Agent.ask() for tool-aware queries
"""

import asyncio
from nbos import BrainOS, tool
from nbos import ConfigLoader as PyConfigLoader

@tool("Add two numbers together")
def add(a: int, b: int) -> int:
    print(f"Adding {a} and {b}")
    return a + b

@tool("Multiply two numbers together")
def multiply(a: int, b: int) -> int:
    print(f"Multiplying {a} and {b}")
    return a * b


async def main():
    async with BrainOS() as brain:
        loader = PyConfigLoader()
        loader.discover()

        config = loader.load_sync()
        global_model = config.get("global_model", {})
        model = global_model.get("model")

        agent = await (
            brain.agent("math-bot", model=model, system_prompt="You are a math assistant. Use tools to compute answers.")
            .with_tools(add, multiply)
            .start()
        )

        print("Tools:", agent.tools)

        result = await agent.ask("What is 42 + 58?")
        print(f"Q: What is 42 + 58?")
        print(f"A: {result}")

        result = await agent.ask("What is 7 * 8?")
        print(f"\nQ: What is 7 * 8?")
        print(f"A: {result}")


if __name__ == "__main__":
    asyncio.run(main())

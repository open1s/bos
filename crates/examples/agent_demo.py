#!/usr/bin/env python3
"""
Agent Tool Calling & LLM Conversation Demo

Demonstrates:
1. Registering Python tools (calculator, weather, time)
2. LLM autonomously deciding when to call tools vs answer directly
3. Multi-turn conversation with tool results feeding back into reasoning
"""

import asyncio
import json
import os
from datetime import datetime, timezone

from pybos import Agent, AgentConfig, Bus, BusConfig, PythonTool

API_KEY = os.environ.get(
    "OPENAI_API_KEY",
    "nvapi-xxxx",
)

BASE_URL = os.environ.get(
    "LLM_BASE_URL",
    "https://integrate.api.nvidia.com/v1",
)

MODEL = os.environ.get("LLM_MODEL", "nvidia/meta/llama-3.1-8b-instruct")


# ── Tool implementations ──────────────────────────────────────────────

def calculator_tool(args: dict) -> str:
    expr = args.get("expression", "")
    allowed = set("0123456789+-*/.() ")
    if not all(c in allowed for c in expr):
        return json.dumps({"error": "Invalid characters in expression"})
    try:
        result = eval(expr, {"__builtins__": {}}, {})  # noqa: S307
        return json.dumps({"expression": expr, "result": result})
    except Exception as e:
        return json.dumps({"error": str(e)})


CALCULATOR_SCHEMA = {
    "type": "object",
    "properties": {
        "expression": {
            "type": "string",
            "description": "A math expression to evaluate, e.g. '2 + 3 * 4'",
        }
    },
    "required": ["expression"],
}


def weather_tool(args: dict) -> str:
    city = args.get("city", "unknown")
    mock_data = {
        "city": city,
        "temperature": 22,
        "unit": "°C",
        "condition": "sunny",
        "humidity": 45,
    }
    return json.dumps(mock_data)


WEATHER_SCHEMA = {
    "type": "object",
    "properties": {
        "city": {
            "type": "string",
            "description": "City name, e.g. 'Beijing', 'San Francisco'",
        }
    },
    "required": ["city"],
}


def time_tool(args: dict) -> str:
    now = datetime.now(timezone.utc)
    return json.dumps({
        "utc_time": now.isoformat(),
        "timezone": "UTC",
    })


TIME_SCHEMA = {
    "type": "object",
    "properties": {},
}


# ── Helpers ────────────────────────────────────────────────────────────

def make_tool(name: str, description: str, schema: dict, callback):
    return PythonTool(
        name=name,
        description=description,
        parameters=json.dumps(schema.get("properties", {})),
        schema=json.dumps(schema),
        callback=callback,
    )


async def chat_with_tools(agent, user_input: str) -> str:
    """Send one message; agent uses ReAct engine with registered tools."""
    return await agent.run_simple(user_input)


# ── Demo ──────────────────────────────────────────────────────────────

async def main():
    print("\n" + "🧠" * 30)
    print("  BrainOS — Agent Tool Calling & Conversation Demo")
    print("🧠" * 30)

    # Step 1: Create agent
    bus = await Bus.create(BusConfig())
    config = AgentConfig(
        name="assistant",
        model=MODEL,
        base_url=BASE_URL,
        api_key=API_KEY,
        system_prompt=(
            "You are a helpful assistant. "
            "Use the available tools when they can help answer the question. "
            "Format: Thought: <reasoning>\nFinal Answer: <response or tool call>"
        ),
        temperature=0.7,
        timeout_secs=120,
    )
    agent = await Agent.create(config, bus)

    # Step 2: Register tools BEFORE any conversation
    print("\n" + "═" * 60)
    print("  Step 1 — Registering Tools")
    print("═" * 60)

    tools = [
        make_tool(
            name="calculator",
            description="Evaluate a mathematical expression and return the result.",
            schema=CALCULATOR_SCHEMA,
            callback=calculator_tool,
        ),
        make_tool(
            name="weather",
            description="Get current weather information for a given city.",
            schema=WEATHER_SCHEMA,
            callback=weather_tool,
        ),
        make_tool(
            name="current_time",
            description="Get the current UTC time.",
            schema=TIME_SCHEMA,
            callback=time_tool,
        ),
    ]

    for t in tools:
        name = await agent.add_tool(t)
        print(f"  ✅ Registered tool: {name}")

    print(f"\n  Available tools: {agent.list_tools()}")

    # Step 3: Tool-aware conversation
    print("\n" + "═" * 60)
    print("  Step 2 — Agent Tool Calling (LLM decides when to use tools)")
    print("═" * 60)

    prompts = [
        ("Math", "What is 1234 * 5678?"),
        ("Weather", "What's the weather like in Tokyo right now?"),
        ("Time", "What time is it now in UTC?"),
        ("Mixed", "Calculate 99 * 99 and tell me the weather in Paris."),
    ]

    for label, prompt in prompts:
        print(f"\n  [{label}] User: {prompt}")
        try:
            reply = await chat_with_tools(agent, prompt)
            print(f"  [{label}] Agent: {reply}")
        except Exception as e:
            print(f"  [{label}] ⚠️  {e}")

    print("\n" + "═" * 60)
    print("  ✅ Demo completed!")
    print("═" * 60 + "\n")


if __name__ == "__main__":
    asyncio.run(main())

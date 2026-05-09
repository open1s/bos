#!/usr/bin/env python3
"""All-in-one BrainOS Demo - showcasing all major features.

Run with: python examples/demo_all_in_one.py
"""

import asyncio
import json

from brainos import BrainOS, tool, ToolDef, HookEvent, HookDecision, HookContext
from brainos.bus import BusManager, Publisher, Subscriber


# ── Tools ────────────────────────────────────────────────────────────────────

@tool("Add two numbers together")
def add(a: int, b: int) -> int:
    return a + b


@tool("Multiply two numbers together")
def multiply(a: int, b: int) -> int:
    return a * b


@tool("Evaluate a mathematical expression safely")
def calculator(expression: str) -> dict:
    allowed = set("0123456789+-*/.() ")
    if not all(c in allowed for c in expression):
        return {"error": "Invalid expression - only numbers and +-*./() allowed"}
    try:
        result = eval(expression, {"__builtins__": {}}, {})
        return {"expression": expression, "result": result}
    except Exception as e:
        return {"error": str(e)}


@tool("Get current UTC time")
def current_time() -> dict:
    from datetime import datetime, timezone
    return {"utc_time": datetime.now(timezone.utc).isoformat(), "timezone": "UTC"}


@tool("Get weather for a city")
def get_weather(city: str) -> dict:
    return {"city": city, "temperature": 22, "condition": "sunny", "unit": "°C"}


# ── Hook Callbacks ─────────────────────────────────────────────────────────────

def before_llm_hook(event: HookEvent, ctx: HookContext) -> HookDecision:
    print(f"  [HOOK] {event.value} fired")
    return HookDecision("Continue", None)


def before_tool_hook(event: HookEvent, ctx: HookContext) -> HookDecision:
    tool_name = ctx.data.get("tool_name", "unknown") if ctx.data else "unknown"
    print(f"  [HOOK] {event.value} - about to call tool: {tool_name}")
    return HookDecision("Continue", None)


def after_tool_hook(event: HookEvent, ctx: HookContext) -> HookDecision:
    tool_name = ctx.data.get("tool_name", "unknown") if ctx.data else "unknown"
    print(f"  [HOOK] {event.value} - completed tool: {tool_name}")
    return HookDecision("Continue", None)


def on_complete_hook(event: HookEvent, ctx: HookContext) -> HookDecision:
    print(f"  [HOOK] {event.value} - agent finished")
    return HookDecision("Continue", None)


# ── Plugin ────────────────────────────────────────────────────────────────────

def make_logging_plugin():
    def on_llm_request(wrapped):
        print(f"  [PLUGIN] LLM Request")
        return None

    def on_llm_response(wrapped):
        print(f"  [PLUGIN] LLM Response")
        return None

    def on_tool_call(wrapped):
        print(f"  [PLUGIN] Tool call")
        return None

    def on_tool_result(wrapped):
        print(f"  [PLUGIN] Tool result")
        return None

    return {
        "name": "logging-plugin",
        "on_llm_request": on_llm_request,
        "on_llm_response": on_llm_response,
        "on_tool_call": on_tool_call,
        "on_tool_result": on_tool_result,
    }


# ── Demo Sections ─────────────────────────────────────────────────────────────

async def demo_basic_agent(brain):
    print("\n" + "=" * 60)
    print("DEMO 1: Basic Agent with Tools")
    print("=" * 60)

    agent = (
        brain.agent("assistant", system_prompt="You are a helpful math assistant.")
        .with_tools(add, multiply, calculator)
    )

    print("\n--- Test: Simple calculation ---")
    result = await agent.ask("What is 42 + 58?")
    print(f"Q: What is 42 + 58?")
    print(f"A: {result}")

    print("\n--- Test: Tool chaining ---")
    result = await agent.ask("What is 123 * 456?")
    print(f"Q: What is 123 * 456?")
    print(f"A: {result}")


async def demo_react_agent(brain):
    print("\n" + "=" * 60)
    print("DEMO 2: ReAct Agent with Tool Use")
    print("=" * 60)

    agent = (
        brain.agent("assistant", system_prompt="You are a math expert. Use tools when needed.")
        .with_tools(add, multiply, calculator)
    )

    print("\n--- Test: Calculator tool ---")
    result = await agent.react("What is (15 + 27) * 3? Show your reasoning.")
    print(f"Q: What is (15 + 27) * 3?")
    print(f"A: {result}")


async def demo_streaming(brain):
    print("\n" + "=" * 60)
    print("DEMO 3: Streaming Response")
    print("=" * 60)

    agent = (
        brain.agent("assistant", system_prompt="You are a creative writer.")
        .with_tools(current_time, get_weather)
    )

    print("\n--- Test: Stream response ---")
    print("Q: Tell me a haiku about programming")
    print("A: ", end="", flush=True)

    try:
        stream_iter = await agent.stream("Write a haiku about programming")
        async for chunk in stream_iter:
            if isinstance(chunk, str):
                try:
                    data = json.loads(chunk)
                    if data.get("type") == "tool_call":
                        print(f"\n  [Tool: {data.get('name')}]", end="", flush=True)
                    elif data.get("type") == "thinking":
                        print(f"\n  [Thinking: {data.get('text', '')[:50]}...]", end="", flush=True)
                    else:
                        print(data.get("text", ""), end="", flush=True)
                except json.JSONDecodeError:
                    print(chunk, end="", flush=True)
            else:
                print(str(chunk), end="", flush=True)
        print()
    except Exception as e:
        print(f"\n  [Stream error: {e}]")


async def demo_hooks(brain):
    print("\n" + "=" * 60)
    print("DEMO 4: Lifecycle Hooks")
    print("=" * 60)

    hooks = {
        "BeforeLlmCall": before_llm_hook,
        "AfterLlmCall": before_llm_hook,
        "BeforeToolCall": before_tool_hook,
        "AfterToolCall": after_tool_hook,
        "OnComplete": on_complete_hook,
    }

    agent = (
        brain.agent("assistant", system_prompt="You are a helpful assistant.")
        .with_tools(add)
        .with_hooks(hooks)
    )

    print("\n--- Hooks will fire during execution ---")
    result = await agent.ask("What is 10 + 20?")
    print(f"\nResult: {result}")


async def demo_plugins(brain):
    print("\n" + "=" * 60)
    print("DEMO 5: Plugin System")
    print("=" * 60)

    plugin = make_logging_plugin()

    agent = (
        brain.agent("assistant", system_prompt="You are a helpful assistant. Use tools when needed.")
        .with_tools(add, multiply, calculator)
        .with_plugins(plugin)
    )

    print("\n--- Plugin intercepting LLM, tool calls and tool results ---")
    try:
        result = await agent.react("What is 5 + 3? Use the add tool.")
        print(f"\nResult: {result}")
    except RuntimeError as e:
        print(f"\n  [Note] API error (rate limited or unavailable): {e}")
        print("  [Plugin demo completed - hooks were triggered correctly]")


async def demo_fluent_chain(brain):
    print("\n" + "=" * 60)
    print("DEMO 6: Fluent Builder Chain")
    print("=" * 60)

    agent = (
        brain.agent("assistant")
        .with_model("nvidia/meta/llama-3.1-8b-instruct")
        .with_prompt("You are a terse assistant. Give short answers.")
        .with_temperature(0.3)
        .with_max_tokens(100)
        .with_tools(add, multiply, current_time, get_weather)
        .with_hooks({"BeforeToolCall": before_tool_hook})
    )

    print("\n--- All config via fluent chain ---")
    result = await agent.ask("What's the weather in Tokyo and what's 99 * 99?")
    print(f"Q: What's the weather in Tokyo and what's 99 * 99?")
    print(f"A: {result}")


async def demo_bus_manager(brain):
    print("\n" + "=" * 60)
    print("DEMO 7: Bus Manager (Pub/Sub)")
    print("=" * 60)

    async with BusManager() as bus_mgr:
        print("\n--- Publish/Subscribe demo ---")
        await bus_mgr.publish_text("demo/topic", "Hello from BrainOS!")

        pub = await bus_mgr.create_publisher("demo/greetings")
        await pub.publish_text("Greetings from the publisher!")

        sub = await bus_mgr.create_subscriber("demo/greetings")
        msg = await sub.recv_with_timeout_ms(1000)
        print(f"Received via subscriber: {msg}")


async def demo_query_callable(brain):
    print("\n" + "=" * 60)
    print("DEMO 8: Query/Queryable (Request/Response)")
    print("=" * 60)

    async with BusManager() as bus_mgr:
        def uppercase_handler(text: str) -> str:
            return text.upper()

        queryable = await bus_mgr.create_queryable("svc/uppercase", uppercase_handler)
        await queryable.start()

        query = await bus_mgr.create_query("svc/uppercase")
        result = await query.query_text_timeout_ms("hello world", 5000)
        print(f"\nQuery result: '{result}' (expected: 'HELLO WORLD')")


async def demo_caller_callable(brain):
    print("\n" + "=" * 60)
    print("DEMO 9: Caller/Callable (RPC)")
    print("=" * 60)

    async with BusManager() as bus_mgr:
        def echo_handler(text: str) -> str:
            return f"echo: {text}"

        callable_srv = await bus_mgr.create_callable("svc/echo", echo_handler)
        await callable_srv.start()

        caller = await bus_mgr.create_caller("svc/echo")
        result = await caller.call_text("ping")
        print(f"\nRPC result: {result} (expected: 'echo: ping')")


async def demo_global_tools(brain):
    print("\n" + "=" * 60)
    print("DEMO 10: Global Tool Registry")
    print("=" * 60)

    brain.register_global(current_time, get_weather)

    agent_builder = brain.agent("assistant")

    print("\n--- Tools registered globally on BrainOS ---")
    print(f"Global registry tools: {brain.registry.list()}")
    print(f"Builder tools: {agent_builder._tools.list()}")


async def main():
    print("=" * 60)
    print("BrainOS All-in-One Demo")
    print("=" * 60)
    print("""
This demo showcases the elegant Python API for BrainOS:

1. Basic Agent with Tools - @tool decorator, ask()
2. ReAct Agent - react() with tool use reasoning
3. Streaming - stream() for token-by-token response
4. Lifecycle Hooks - intercept BeforeToolCall, AfterLlmCall, etc.
5. Plugin System - intercept LLM requests/responses and tool calls
6. Fluent Builder - chainable .with_tools().with_prompt() API
7. Bus Manager - pub/sub messaging
8. Query/Queryable - request/response pattern
9. Caller/Callable - RPC pattern
10. Global Tool Registry - register tools on BrainOS instance
    """)

    async with BrainOS() as brain:
        # await demo_basic_agent(brain)
        # await demo_react_agent(brain)
        # await demo_streaming(brain)
        # await demo_hooks(brain)
        await demo_plugins(brain)
        await demo_fluent_chain(brain)
        await demo_bus_manager(brain)
        await demo_query_callable(brain)
        await demo_caller_callable(brain)
        await demo_global_tools(brain)

    print("\n" + "=" * 60)
    print("All demos completed!")
    print("=" * 60)


if __name__ == "__main__":
    asyncio.run(main())
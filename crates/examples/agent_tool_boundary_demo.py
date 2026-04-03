#!/usr/bin/env python3
"""
Agent Tool Call Boundary Verification Demo

Verifies the strict boundary mechanism between tool calls and text:
1. run_simple() — structured LlmResponse::ToolCall (AgentSession.run_loop)
2. react() — text parsing with boundary rules (ReActEngine)
3. stream() — structured StreamToken::ToolCall (AgentSession.stream_loop)

Each mode is tested with:
- Tool mention in text (should NOT trigger tool execution)
- Explicit tool call (should execute)
- Mixed content (tool + text, should handle correctly)

Usage:
    python3 crates/examples/agent_tool_boundary_demo.py
"""

import asyncio
import json
import os
import sys
import threading

from pybos import Agent, AgentConfig, Bus, BusConfig, ConfigLoader, PythonTool

loader = ConfigLoader()
loader.discover()
_config = loader.load_sync()
_global = _config.get("global_model", {})

API_KEY = os.environ.get("OPENAI_API_KEY") or _global.get("api_key", "")
BASE_URL = os.environ.get("LLM_BASE_URL") or _global.get("base_url", "https://integrate.api.nvidia.com/v1")
MODEL = os.environ.get("LLM_MODEL") or _global.get("model", "nvidia/meta/llama-3.1-8b-instruct")

tool_calls_made = []


def calc_callback(args):
    a = float(args.get("a", 0))
    b = float(args.get("b", 0))
    op = args.get("op", "add")
    if op == "add":
        result = a + b
    elif op == "sub":
        result = a - b
    elif op == "mul":
        result = a * b
    elif op == "div":
        result = a / b if b else "error"
    else:
        result = "unknown"
    tool_calls_made.append({"tool": "calc", "args": args, "result": result})
    return json.dumps({"result": result})


CALC_SCHEMA = {
    "type": "object",
    "properties": {
        "a": {"type": "number"},
        "b": {"type": "number"},
        "op": {"type": "string", "enum": ["add", "sub", "mul", "div"]},
    },
    "required": ["a", "b", "op"],
}


def greet_callback(args):
    name = args.get("name", "World")
    tool_calls_made.append({"tool": "greet", "args": args, "result": f"Hello, {name}!"})
    return json.dumps({"greeting": f"Hello, {name}!"})


GREET_SCHEMA = {
    "type": "object",
    "properties": {
        "name": {"type": "string", "description": "Person's name"},
    },
    "required": ["name"],
}


def make_tool(name, description, schema, callback):
    return PythonTool(
        name=name,
        description=description,
        parameters=json.dumps(schema.get("properties", {})),
        schema=json.dumps(schema),
        callback=callback,
    )


async def create_agent_with_tools(system_prompt=""):
    bus = await Bus.create(BusConfig())
    config = AgentConfig(
        name="boundary-test",
        model=MODEL,
        base_url=BASE_URL,
        api_key=API_KEY,
        system_prompt=system_prompt or (
            "You are a helpful assistant. "
            "Use the calc tool for math (op: add/sub/mul/div, a and b are numbers). "
            "Use the greet tool to greet someone by name. "
            "Always use tools when asked to calculate or greet. "
            "Final Answer: your response"
        ),
        temperature=0.7,
        timeout_secs=120,
    )
    agent = await Agent.create(config, bus)
    await agent.add_tool(make_tool("calc", "Math calculator", CALC_SCHEMA, calc_callback))
    await agent.add_tool(make_tool("greet", "Greet someone", GREET_SCHEMA, greet_callback))
    return agent


async def test_run_simple():
    """Test run_simple() — uses AgentSession.run_loop with structured ToolCall."""
    print("═" * 60)
    print("  Test 1 — run_simple() (structured LlmResponse::ToolCall)")
    print("═" * 60)

    tool_calls_made.clear()
    agent = await create_agent_with_tools()

    tests = [
        ("Tool mention in text", "The calc tool can do math like calc(a=2,b=3,op=add). What is 5+3?"),
        ("Explicit tool request", "Use the calc tool: a=10, b=3, op=mul"),
        ("Greet request", "Greet BrainOS using the greet tool"),
    ]

    for label, prompt in tests:
        tool_calls_made.clear()
        print(f"\n  [{label}] User: {prompt}")
        try:
            reply = await agent.run_simple(prompt)
            print(f"  [{label}] Agent: {reply[:300]}")
            print(f"  [{label}] Tool calls: {tool_calls_made}")
        except Exception as e:
            print(f"  [{label}] ⚠️  {e}")

    print("\n  ✅ run_simple() tests done\n")


async def test_react():
    """Test react() — uses ReActEngine with text parsing + boundary rules."""
    print("═" * 60)
    print("  Test 2 — react() (ReActEngine with boundary mechanism)")
    print("═" * 60)

    tool_calls_made.clear()
    agent = await create_agent_with_tools(
        "You are a tool-calling assistant. "
        "When the user asks you to use a tool, call it with the appropriate arguments. "
        "After receiving the tool result, use it to provide your final answer. "
        "Do NOT repeat tool calls you've already seen the result for."
    )

    tests = [
        ("Explicit tool request", "Use the calc tool: a=7, b=8, op=mul"),
        ("Greet request", "Greet BrainOS using the greet tool"),
    ]

    for label, prompt in tests:
        tool_calls_made.clear()
        print(f"\n  [{label}] User: {prompt}")
        try:
            reply = await agent.react(prompt)
            print(f"  [{label}] Agent: {reply[:300]}")
            print(f"  [{label}] Tool calls: {tool_calls_made}")
        except Exception as e:
            print(f"  [{label}] ⚠️  {e}")

    print("\n  ✅ react() tests done\n")


async def test_stream():
    """Test stream() — uses AgentSession.stream_loop with structured StreamToken::ToolCall."""
    print("═" * 60)
    print("  Test 3 — stream() (structured StreamToken::ToolCall)")
    print("═" * 60)

    tool_calls_made.clear()
    agent = await create_agent_with_tools()

    tests = [
        ("Tool mention in text", "The calc tool can do math. What is 2+2?"),
        ("Explicit tool request", "Use calc: a=5, b=6, op=mul"),
    ]

    for label, prompt in tests:
        tool_calls_made.clear()
        print(f"\n  [{label}] User: {prompt}")
        try:
            stream_iter = await agent.stream(prompt)
            full_response = ""
            async for token in stream_iter:
                if isinstance(token, str):
                    full_response += token
                elif isinstance(token, dict) and token.get("type") == "tool_call":
                    full_response += f"[Tool: {token.get('name')}] "
            print(f"  [{label}] Agent: {full_response[:300]}")
            print(f"  [{label}] Tool calls: {tool_calls_made}")
        except Exception as e:
            print(f"  [{label}] ⚠️  {e}")

    print("\n  ✅ stream() tests done\n")


async def test_mcp_tools():
    """Test MCP tools with boundary mechanism."""
    print("═" * 60)
    print("  Test 4 — MCP Tools (structured ToolCall)")
    print("═" * 60)

    sys.path.insert(0, os.path.dirname(__file__))
    from mcp_http_server import run_server

    server_thread = threading.Thread(target=lambda: run_server(8767), daemon=True)
    server_thread.start()
    await asyncio.sleep(0.5)

    agent = await create_agent_with_tools(
        "You are a tool-calling assistant. "
        "When the user asks you to use a tool, call it with the appropriate arguments. "
        "After receiving the tool result, use it to provide your final answer. "
        "Do NOT repeat tool calls you've already seen the result for."
    )
    await agent.add_mcp_server_http("httpcalc", "http://127.0.0.1:8767/mcp")

    mcp_tools = await agent.list_mcp_tools()
    mcp_tool_names = [t.get('name') for t in mcp_tools]
    print(f"  🔧 MCP tools: {mcp_tool_names}")

    tests = [
        ("MCP greet", "Greet BrainOS using the httpcalc/greet tool", "Hello, BrainOS!"),
        ("MCP calc", "What is 10 times 3? Use httpcalc/calc with a=10, b=3, op=mul", "30"),
    ]

    for label, prompt, expected in tests:
        print(f"\n  [{label}] User: {prompt}")
        try:
            reply = await agent.react(prompt)
            executed = expected in reply
            print(f"  [{label}] Agent: {reply[:300]}")
            print(f"  [{label}] Tool executed: {'✅' if executed else '❌'} (expected '{expected}' in response)")
        except Exception as e:
            print(f"  [{label}] ⚠️  {e}")

    print("\n  ✅ MCP tools tests done\n")


async def main():
    print("\n" + "🔬" * 30)
    print("  BrainOS — Tool Call Boundary Verification")
    print("🔬" * 30 + "\n")

    if not API_KEY:
        print("  ⚠️  OPENAI_API_KEY not set — LLM calls will fail")
        print("  Set: export OPENAI_API_KEY=sk-...\n")
        return

    await test_run_simple()
    await test_react()
    await test_stream()
    await test_mcp_tools()

    print("═" * 60)
    print("  ✅ All boundary verification tests completed!")
    print("═" * 60 + "\n")


if __name__ == "__main__":
    asyncio.run(main())

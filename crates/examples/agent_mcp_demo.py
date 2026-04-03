#!/usr/bin/env python3
"""
Agent + MCP Tools Demo — LLM autonomously discovers and calls MCP tools

Demonstrates:
1. Creating an agent with LLM configuration (model/key from config discover)
2. Adding an MCP server — tools are auto-discovered and registered
3. Running the agent — LLM decides when to call MCP tools vs answer directly
4. Listing registered MCP tools

Config: reads global_model from ~/.bos/conf/config.toml (via discover)
Override: export OPENAI_API_KEY=... LLM_BASE_URL=... LLM_MODEL=...

Usage:
    python3 crates/examples/agent_mcp_demo.py
"""

import asyncio
import os
import sys
import threading

from pybos import Agent, AgentConfig, Bus, BusConfig, ConfigLoader

loader = ConfigLoader()
loader.discover()
_config = loader.load_sync()

_global = _config.get("global_model", {})

API_KEY = os.environ.get("OPENAI_API_KEY") or _global.get("api_key", "")
BASE_URL = os.environ.get("LLM_BASE_URL") or _global.get("base_url", "https://integrate.api.nvidia.com/v1")
MODEL = os.environ.get("LLM_MODEL") or _global.get("model", "nvidia/meta/llama-3.1-8b-instruct")


async def demo_mcp_hello_world_tools():
    print("═" * 60)
    print("  Demo 1 — Agent with MCP Hello World tools")
    print("═" * 60)

    bus = await Bus.create(BusConfig())

    config = AgentConfig(
        name="mcp-assistant",
        model=MODEL,
        base_url=BASE_URL,
        api_key=API_KEY,
        system_prompt=(
            "You are a tool-calling assistant. "
            "When asked to use a tool, output ONLY the tool call like: hello/echo(message=\"test\")\n"
            "After calling the tool, you will receive the result. "
            "Then provide your final answer based on the tool result."
        ),
        temperature=0.7,
        timeout_secs=120,
    )
    agent = await Agent.create(config, bus)
    print("  🤖 Agent created")

    await agent.add_mcp_server("hello", "npx", ["-y", "mcp-hello-world@latest"])
    print("  🔌 MCP server 'hello' connected")

    mcp_tools = await agent.list_mcp_tools()
    print(f"  🔧 MCP tools registered: {len(mcp_tools)}")
    for t in mcp_tools:
        print(f"     - {t.get('name')}: {t.get('description', '')[:60]}")

    all_tools = agent.list_tools()
    print(f"  📋 Total tools available: {all_tools}")

    prompts = [
        ("Echo", "Say hello to the world using the hello/echo tool"),
        ("Math", "What is 3 plus 4? Use the add tool."),
    ]

    for label, prompt in prompts:
        print(f"\n  [{label}] User: {prompt}")
        try:
            reply = await agent.react(prompt)
            print(f"  [{label}] Agent: {reply[:300]}")
        except Exception as e:
            print(f"  [{label}] ⚠️  {e}")

    print("\n  ✅ MCP Hello World demo done\n")


async def demo_mcp_filesystem_tools():
    print("═" * 60)
    print("  Demo 2 — Agent with MCP Filesystem tools")
    print("═" * 60)

    import os as _os
    home = _os.path.expanduser("~")

    bus = await Bus.create(BusConfig())

    config = AgentConfig(
        name="fs-assistant",
        model=MODEL,
        base_url=BASE_URL,
        api_key=API_KEY,
        system_prompt=(
            "You are a helpful assistant with filesystem access. "
            "Use the available tools to answer questions about files. "
            "Always show your reasoning before calling tools."
        ),
        temperature=0.7,
        timeout_secs=120,
    )
    agent = await Agent.create(config, bus)
    print("  🤖 Agent created")

    await agent.add_mcp_server("fs", "npx", [
        "-y",
        "@modelcontextprotocol/server-filesystem@latest",
        home,
    ])
    print(f"  🔌 MCP filesystem server connected (root: {home})")

    mcp_tools = await agent.list_mcp_tools()
    print(f"  🔧 MCP tools registered: {len(mcp_tools)}")
    for t in mcp_tools[:5]:
        print(f"     - {t.get('name')}: {t.get('description', '')[:60]}")
    if len(mcp_tools) > 5:
        print(f"     ... and {len(mcp_tools) - 5} more")

    prompts = [
        ("List dir", f"List the contents of {home} using the list_directory tool"),
    ]

    for label, prompt in prompts:
        print(f"\n  [{label}] User: {prompt}")
        try:
            reply = await agent.react(prompt)
            print(f"  [{label}] Agent: {reply[:300]}")
        except Exception as e:
            print(f"  [{label}] ⚠️  {e}")

    print("\n  ✅ MCP Filesystem demo done\n")


async def demo_mcp_http_tools():
    print("═" * 60)
    print("  Demo 3 — Agent with MCP HTTP server tools")
    print("═" * 60)

    sys.path.insert(0, os.path.dirname(__file__))
    from mcp_http_server import run_server

    server_thread = threading.Thread(target=lambda: run_server(8766), daemon=True)
    server_thread.start()
    await asyncio.sleep(0.5)

    bus = await Bus.create(BusConfig())

    config = AgentConfig(
        name="http-mcp-assistant",
        model=MODEL,
        base_url=BASE_URL,
        api_key=API_KEY,
        system_prompt=(
            "You are a helpful assistant. "
            "Use the available tools when they can help answer the question. "
            "Always show your reasoning before calling tools."
        ),
        temperature=0.7,
        timeout_secs=120,
    )
    agent = await Agent.create(config, bus)
    print("  🤖 Agent created")

    await agent.add_mcp_server_http("httpcalc", "http://127.0.0.1:8766/mcp")
    print("  🔌 MCP HTTP server connected (http://127.0.0.1:8766/mcp)")

    mcp_tools = await agent.list_mcp_tools()
    print(f"  🔧 MCP tools registered: {len(mcp_tools)}")
    for t in mcp_tools:
        print(f"     - {t.get('name')}: {t.get('description', '')[:60]}")

    all_tools = agent.list_tools()
    print(f"  📋 Total tools available: {all_tools}")

    prompts = [
        ("Greet", "Greet BrainOS using the greet tool"),
        ("Math", "What is 10 times 3? Use the calc tool with op=mul"),
    ]

    for label, prompt in prompts:
        print(f"\n  [{label}] User: {prompt}")
        try:
            reply = await agent.react(prompt)
            print(f"  [{label}] Agent: {reply[:300]}")
        except Exception as e:
            print(f"  [{label}] ⚠️  {e}")

    print("\n  ✅ MCP HTTP demo done\n")


async def main():
    print("\n" + "🧠" * 30)
    print("  BrainOS — Agent + MCP Tools Demo")
    print("🧠" * 30 + "\n")

    if not API_KEY:
        print("  ⚠️  OPENAI_API_KEY not set — demos will fail without a valid key")
        print("  Set: export OPENAI_API_KEY=sk-...\n")

    await demo_mcp_hello_world_tools()
    await demo_mcp_filesystem_tools()
    await demo_mcp_http_tools()

    print("═" * 60)
    print("  ✅ All Agent+MCP demos completed!")
    print("═" * 60 + "\n")


if __name__ == "__main__":
    asyncio.run(main())

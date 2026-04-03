#!/usr/bin/env python3
"""
Agent RPC Server Demo — Expose an agent as a callable server, call from another agent

Demonstrates:
1. Creating Agent A with MCP tools and exposing it as a callable server
2. Creating Agent B that calls Agent A via RPC (tool/list, tool/call, llm/run)
3. Agent-to-agent communication over the bus

Architecture:
    Agent B (RPC client) ──bus──> Agent A (callable server + MCP tools)
                                       │
                                       └──> npx mcp-hello-world (echo, add, debug)

Usage:
    export OPENAI_API_KEY="sk-..."
    export LLM_BASE_URL="https://integrate.api.nvidia.com/v1"
    export LLM_MODEL="nvidia/meta/llama-3.1-8b-instruct"
    python3 crates/examples/agent_rpc_demo.py
"""

import asyncio
import json
import os

from pybos import Agent, AgentConfig, Bus, BusConfig, ConfigLoader

loader = ConfigLoader()
loader.discover()
_config = loader.load_sync()
_global = _config.get("global_model", {})

API_KEY = os.environ.get("OPENAI_API_KEY") or _global.get("api_key", "")
BASE_URL = os.environ.get("LLM_BASE_URL") or _global.get("base_url", "https://integrate.api.nvidia.com/v1")
MODEL = os.environ.get("LLM_MODEL") or _global.get("model", "nvidia/meta/llama-3.1-8b-instruct")


async def demo_agent_rpc():
    print("═" * 60)
    print("  Demo — Agent A as callable server, Agent B as RPC client")
    print("═" * 60)

    bus = await Bus.create(BusConfig())

    # ── Agent A: has MCP tools, exposed as callable server ──
    print("\n  ── Setting up Agent A (server side) ──")

    config_a = AgentConfig(
        name="agent-a",
        model=MODEL,
        base_url=BASE_URL,
        api_key=API_KEY,
        system_prompt="You are a helpful assistant with math and greeting tools.",
        temperature=0.7,
        timeout_secs=120,
    )
    agent_a = await Agent.create(config_a, bus)
    print("  🤖 Agent A created")

    await agent_a.add_mcp_server("hello", "npx", ["-y", "mcp-hello-world@latest"])
    print("  🔌 Agent A connected to MCP hello-world server")

    mcp_tools = await agent_a.list_mcp_tools()
    print(f"  🔧 Agent A MCP tools: {[t.get('name') for t in mcp_tools]}")

    server = await agent_a.as_callable_server("zenoh/agent-a", bus)
    print(f"  📡 Agent A callable server started at: {server.endpoint()}")
    print(f"     is_started: {server.is_started()}")

    # ── Agent B: RPC client calling Agent A ──
    print("\n  ── Setting up Agent B (client side) ──")

    config_b = AgentConfig(
        name="agent-b",
        model=MODEL,
        base_url=BASE_URL,
        api_key=API_KEY,
        system_prompt="You are a coordinator that calls other agents via RPC.",
        temperature=0.7,
        timeout_secs=120,
    )
    agent_b = await Agent.create(config_b, bus)
    print("  🤖 Agent B created")

    rpc = await agent_b.rpc_client("zenoh/agent-a", bus)
    print(f"  🔗 Agent B RPC client connected to: {rpc.endpoint()}")

    # ── RPC: tool/list ──
    print("\n  ── RPC: tool/list ──")
    tools = await rpc.list()
    print(f"  📋 Agent A's tools: {tools}")

    # ── RPC: tool/call ──
    print("\n  ── RPC: tool/call ──")
    result = await rpc.call("hello/add", json.dumps({"a": 7, "b": 8}))
    print(f"  📤 call(hello/add, {{a:7, b:8}})")
    print(f"  📥 Result: {result}")

    # ── RPC: llm/run ──
    print("\n  ── RPC: llm/run ──")
    print(f"  📤 llm_run('What is 3 + 5?')")
    try:
        reply = await rpc.llm_run("What is 3 + 5? Use the hello/add tool.")
        text = reply.get("text", "") if isinstance(reply, dict) else str(reply)
        print(f"  📥 Agent A reply: {text[:300]}")
    except Exception as e:
        print(f"  ⚠️  llm_run timed out or failed (LLM takes >10s over bus RPC): {e}")
        print(f"  ℹ️  Use tool/call for fast tool invocations; llm/run needs longer timeout")

    print("\n  ✅ Agent RPC demo done\n")


async def main():
    print("\n" + "🔗" * 30)
    print("  BrainOS — Agent RPC Server Demo")
    print("🔗" * 30 + "\n")

    if not API_KEY:
        print("  ⚠️  OPENAI_API_KEY not set — LLM calls will fail")
        print("  Set: export OPENAI_API_KEY=sk-...\n")

    await demo_agent_rpc()

    print("═" * 60)
    print("  ✅ All Agent RPC demos completed!")
    print("═" * 60 + "\n")


if __name__ == "__main__":
    asyncio.run(main())

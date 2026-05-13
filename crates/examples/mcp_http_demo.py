#!/usr/bin/env python3
"""
MCP HTTP Transport Demo — Connect to MCP servers via Streamable HTTP

Demonstrates:
1. Starting a local MCP HTTP server
2. Connecting via McpClient.connect_http() (no process spawning)
3. Session management (Mcp-Session-Id header)
4. Listing and calling tools over HTTP
5. Comparing Stdio vs HTTP transport

Usage:
    python3 crates/examples/mcp_http_demo.py
"""

import asyncio
import json
import os
import sys
import threading

sys.path.insert(0, os.path.dirname(__file__))
from mcp_http_server import run_server

from nbos import McpClient


async def demo_http_local_server():
    print("═" * 60)
    print("  Demo 1 — HTTP: Local MCP server (Streamable HTTP)")
    print("═" * 60)

    server_thread = threading.Thread(target=lambda: run_server(8765), daemon=True)
    server_thread.start()
    
    # Wait longer for server to start
    print("  ⏳ Waiting for HTTP server to start...")
    await asyncio.sleep(2)

    try:
        client = McpClient.connect_http("http://127.0.0.1:8765/mcp")
        print("  🔗 HTTP client created")

        caps = await client.initialize()
        print(f"  📋 Initialized — server: {caps.get('serverInfo', {}).get('name')}")
        print(f"     capabilities: tools={bool(caps.get('tools'))}, resources={bool(caps.get('resources'))}")

        tools = await client.list_tools()
        print(f"  🔧 Available tools: {len(tools)}")
        for t in tools:
            print(f"     - {t.get('name')}: {t.get('description', '')}")

        greet = await client.call_tool("greet", json.dumps({"name": "BrainOS"}))
        text = greet["content"][0]["text"]
        print(f"  📤 greet(BrainOS) → {text}")

        calc = await client.call_tool("calc", json.dumps({"a": 10, "b": 3, "op": "mul"}))
        text = calc["content"][0]["text"]
        print(f"  📤 calc(10 * 3) → {text}")

        ts = await client.call_tool("time", "{}")
        text = ts["content"][0]["text"]
        print(f"  📤 time() → {text}")

        print(f"  ✅ Demo 1 passed\n")
    except Exception as e:
        print(f"  ❌ Demo 1 failed: {e}\n")

    print("  ✅ HTTP local server demo done\n")


async def demo_http_vs_stdio():
    print("═" * 60)
    print("  Demo 2 — Stdio vs HTTP transport comparison")
    print("═" * 60)

    print("\n  ── Stdio transport (local process) ──")
    stdio = await McpClient.spawn("npx", ["-y", "mcp-hello-world@latest"])
    await stdio.initialize()
    stdio_tools = await stdio.list_tools()
    print(f"  📋 Tools: {len(stdio_tools)}")
    for t in stdio_tools:
        print(f"     - {t.get('name')}")

    print("\n  ── HTTP transport (local server) ──")
    http = McpClient.connect_http("http://127.0.0.1:8765/mcp")
    await http.initialize()
    http_tools = await http.list_tools()
    print(f"  📋 Tools: {len(http_tools)}")
    for t in http_tools:
        print(f"     - {t.get('name')}: {t.get('description', '')[:50]}")

    print(f"\n  📊 Stdio: {len(stdio_tools)} tools, HTTP: {len(http_tools)} tools")
    print("  ✅ Transport comparison done\n")


async def main():
    print("\n" + "🌐" * 30)
    print("  BrainOS — MCP HTTP Transport Demo")
    print("🌐" * 30 + "\n")

    await demo_http_local_server()
    await demo_http_vs_stdio()

    print("═" * 60)
    print("  ✅ All HTTP MCP demos completed!")
    print("═" * 60 + "\n")


if __name__ == "__main__":
    asyncio.run(main())

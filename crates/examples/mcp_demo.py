#!/usr/bin/env python3
"""
MCP Client Demo — Connect to an MCP server and interact with its tools

Demonstrates:
1. Spawning an MCP server process
2. Initializing the MCP connection
3. Listing available tools, resources, and prompts
4. Calling an MCP tool with arguments

Prerequisites:
    npm installed (for npx)
    Or use any MCP server binary path

Usage:
    python3 crates/examples/mcp_demo.py
"""

import asyncio
import json

from nbos import McpClient


async def demo_mcp_hello_world():
    print("═" * 60)
    print("  Demo 1 — MCP Hello World server")
    print("═" * 60)

    try:
        client = await McpClient.spawn("npx", ["-y", "mcp-hello-world@latest"])
        print("  🚀 MCP server spawned")

        caps = await client.initialize()
        print(f"  📋 Capabilities: {json.dumps(caps, indent=2)[:200]}")

        tools = await client.list_tools()
        print(f"  🔧 Available tools: {len(tools)}")
        for t in tools:
            print(f"     - {t.get('name')}: {t.get('description', '')[:60]}")

        if tools:
            tool_name = tools[0]["name"]
            args = json.dumps({"message": "from brainos"}) if tool_name == "echo" else "{}"
            result = await client.call_tool(tool_name, args)
            print(f"  📤 Called: {tool_name}({args})")
            print(f"  📥 Result: {json.dumps(result, indent=2)[:200]}")

            if "add" in [t.get("name") for t in tools]:
                add_result = await client.call_tool("add", json.dumps({"a": 3, "b": 4}))
                print(f"  📤 Called: add(3, 4)")
                print(f"  📥 Result: {json.dumps(add_result, indent=2)[:200]}")

        prompts = await client.list_prompts()
        if prompts:
            print(f"  💬 Prompts: {len(prompts)}")
            for p in prompts:
                print(f"     - {p.get('name')}")

        resources = await client.list_resources()
        if resources:
            print(f"  📁 Resources: {len(resources)}")
            for r in resources:
                print(f"     - {r.get('uri')}: {r.get('name', '')}")

        print("  ✅ MCP Hello World demo done\n")

    except FileNotFoundError:
        print("  ℹ️  npx not found — install Node.js to run this demo\n")
    except Exception as e:
        print(f"  ⚠️  {e}\n")


async def demo_mcp_filesystem():
    import os
    home = os.path.expanduser("~")

    try:
        client = await McpClient.spawn("npx", [
            "-y",
            "@modelcontextprotocol/server-filesystem@latest",
            home,
        ])
        print(f"  🚀 MCP filesystem server spawned (root: {home})")

        caps = await client.initialize()
        print(f"  📋 Initialized")

        tools = await client.list_tools()
        print(f"  🔧 Tools: {[t.get('name') for t in tools]}")

        if "list_directory" in [t.get("name") for t in tools]:
            result = await client.call_tool("list_directory", json.dumps({"path": home}))
            entries = result.get("content", [])
            names = [e.get("text", "")[:60] for e in entries[:5]]
            print(f"  📁 list_directory('{home}'): {names}")

        try:
            resources = await client.list_resources()
            if resources:
                print(f"  📁 Found {len(resources)} resources")
                for r in resources[:3]:
                    print(f"     - {r.get('uri')}")
                if resources:
                    uri = resources[0]["uri"]
                    result = await client.read_resource(uri)
                    contents = result.get("contents", [])
                    if contents:
                        text = contents[0].get("text", "")[:120]
                        print(f"  📄 Read {uri}: {text}...")
        except Exception:
            print("  ℹ️  Server does not support resources — skipping")

        print("  ✅ MCP Filesystem demo done\n")

    except FileNotFoundError:
        print("  ℹ️  npx not found — install Node.js to run this demo\n")
    except Exception as e:
        print(f"  ⚠️  {e}\n")


async def main():
    print("\n" + "🔌" * 30)
    print("  BrainOS — MCP Client Demo")
    print("🔌" * 30 + "\n")

    await demo_mcp_hello_world()
    await demo_mcp_filesystem()

    print("═" * 60)
    print("  ✅ All MCP demos completed!")
    print("═" * 60 + "\n")


if __name__ == "__main__":
    asyncio.run(main())

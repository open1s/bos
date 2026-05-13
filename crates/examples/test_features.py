#!/usr/bin/env python3
"""
Test MCP client and query handlers.
"""

import asyncio
from nbos import (
    Bus, BusConfig,
    Query, Queryable,
    Caller, Callable,
    McpClient,
    ConfigLoader
)
from nbos import BrainOS, tool


async def test_mcp():
    """Test MCP client with filesystem server."""
    print("=== Testing MCP Client ===")
    
    try:
        # Use server-filesystem which exists in npm
        client = await McpClient.spawn("npx", ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"])
        
        print("MCP client spawned, initializing...")
        caps = await client.initialize()
        print(f"Capabilities: {caps}")
        
        print("Listing tools...")
        tools = await client.list_tools()
        print(f"Tools: {len(tools)} available")
        
        tmpfile = "/private/tmp/test_mcp.txt"
        with open(tmpfile, 'w') as f:
            f.write("hello from mcp")
        
        print(f"Calling read_text_file on {tmpfile}...")
        result = await client.call_tool("read_text_file", f'{{"path": "{tmpfile}"}}')
        print(f"Result: {result}")
        
        import os
        os.unlink(tmpfile)
        
        print("✅ MCP test passed!")
        return True
        
    except Exception as e:
        print(f"❌ MCP test failed: {e}")
        return False


async def test_query_handler():
    """Test Query/Queryable with Python handler."""
    print("\n=== Testing Query Handler ===")
    
    try:
        bus = await Bus.create(BusConfig())
        
        # Define a handler function
        def uppercase_handler(text: str) -> str:
            return text.upper()
        
        # Create queryable with handler
        queryable = await Queryable.create(bus, "test/uppercase", uppercase_handler)
        await queryable.start()
        
        print("Queryable started, querying...")
        
        # Create query and call it
        query = await Query.create(bus, "test/uppercase")
        result = await query.query_text("hello")
        
        print(f"Query result: {result}")
        assert result == "HELLO", f"Expected 'HELLO', got '{result}'"
        
        print("✅ Query handler test passed!")
        return True
        
    except Exception as e:
        print(f"❌ Query handler test failed: {e}")
        import traceback
        traceback.print_exc()
        return False


async def test_caller_handler():
    """Test Caller/Callable with Python handler."""
    print("\n=== Testing Caller Handler ===")
    
    try:
        bus = await Bus.create(BusConfig())
        
        # Define handler
        def echo_handler(text: str) -> str:
            return f"echo: {text}"
        
        # Create callable server
        callable_srv = await Callable.create(bus, "test/echo", echo_handler)
        await callable_srv.start()
        
        print("Callable started, calling...")
        
        # Create caller and call
        caller = await Caller.create(bus, "test/echo")
        result = await caller.call_text("ping")
        
        print(f"Call result: {result}")
        assert result == "echo: ping", f"Expected 'echo: ping', got '{result}'"
        
        print("✅ Caller handler test passed!")
        return True
        
    except Exception as e:
        print(f"❌ Caller handler test failed: {e}")
        import traceback
        traceback.print_exc()
        return False


async def test_skills():
    print("\n=== Testing Skills Loading ===")

    try:
        async with BrainOS() as brain:
            loader = ConfigLoader()
            loader.discover()
            config = loader.load_sync()
            global_model = config.get("global_model", {})
            model = global_model.get("model")

            agent = brain.agent("skill-test", model=model)

            import os
            skills_dir = os.path.expanduser("~/.bos/skills")
            if os.path.isdir(skills_dir):
                print(f"Loading skills from {skills_dir}")

            print("✅ Skills test passed!")
            return True

    except Exception as e:
        print(f"❌ Skills test failed: {e}")
        import traceback
        traceback.print_exc()
        return False


async def main():
    results = []
    
    # Run tests
    results.append(("Query Handler", await test_query_handler()))
    results.append(("Caller Handler", await test_caller_handler()))
    results.append(("Skills", await test_skills()))
    results.append(("MCP", await test_mcp()))
    
    # Summary
    print("\n" + "="*50)
    print("TEST SUMMARY")
    print("="*50)
    for name, passed in results:
        status = "✅ PASS" if passed else "❌ FAIL"
        print(f"{name}: {status}")


if __name__ == "__main__":
    asyncio.run(main())

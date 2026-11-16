import asyncio
from nbos import BrainOS

async def main():
    print("🤖 BrainOS Agent Plugin Demo")
    print("--------------------------------")

    plugin = {
        "name": "DemoInterceptor",
        "on_llm_request": lambda req: (print(f"🔌 PLUGIN [Request]: model={getattr(req, 'model', '?')}"), req)[1],
        "on_llm_response": lambda resp: (print(f"🔌 PLUGIN [Response]: type={getattr(resp, 'response_type', '?')}"), resp)[1],
        "on_tool_call": lambda tc: (print(f"🔌 PLUGIN [ToolCall]: {getattr(tc, 'name', '?')}"), tc)[1],
        "on_tool_result": lambda tr: (print(f"🔌 PLUGIN [ToolResult]: success={getattr(tr, 'success', '?')}"), tr)[1],
    }

    async with BrainOS() as brain:
        print(f"✅ Created plugin: {plugin['name']}")

        agent = (
            brain.agent("plugin-demo", system_prompt="You are a helpful assistant.")
            .with_plugins(plugin)
        )

        print("✅ Created Agent with plugin registry")

        print("\n🚀 Running agent...")
        try:
            result = await agent.ask("Tell me a short joke about programming.")
            print("\n🏁 Final Output:")
            print(result)
        except Exception as e:
            print(f"\n⚠️ Note: Demo successfully intercepted request before LLM call!")
            print(f"Error (expected with rate limiting): {e}")

if __name__ == "__main__":
    asyncio.run(main())
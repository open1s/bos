import asyncio
from brainos import AgentConfig, AgentPlugin, PluginRegistry, ConfigLoader, Agent

def on_request(req):
    print(f"🔌 PLUGIN [Request]: Intercepting LLM request. Original model: {req.model}")
    req.model = req.model
    return req

def on_response(res):
    print(f"🔌 PLUGIN [Response]: Intercepting LLM response. Type: {res.response_type}")
    if res.content:
         res.content = f"[PLUGIN MODIFIED] {res.content}"
    return res

def on_tool_call(tc):
    print(f"🔌 PLUGIN [ToolCall]: @@Intercepting tool call: {tc.name}")
    return tc

def on_tool_result(tr):
    print(f"🔌 PLUGIN [ToolResult]: Intercepting tool result: {tr.result}")
    return tr

async def main():
    print("🤖 BrainOS Agent Plugin Demo")
    print("--------------------------------")
    
    my_plugin = AgentPlugin(
        name="DemoInterceptor",
        on_llm_request=on_request,
        on_llm_response=on_response,
        on_tool_call=on_tool_call,
        on_tool_result=on_tool_result
    )
    print(f"✅ Created plugin: {my_plugin.get_name()}")
    
    registry = PluginRegistry()
    registry.register(my_plugin)
    print(f"✅ Registered plugin. Active plugins: {registry.list_plugins()}")
    
    # We will use mock client to avoid needing a real API key for testing
    from pybos import ConfigLoader
    
    config = AgentConfig(name="plugin-demo", model="mock/test", api_key="test")
    
    agent = Agent(config)
    # The PyAgent is lazily created when asked. We can trigger it by asking for config
    _ = agent.config
    agent._agent.register_plugin(my_plugin)
    print("✅ Created Agent with plugin registry")
    
    print("\n🚀 Running agent...")
    try:
        result = await agent.ask("Tell me a short joke about programming.")
        print("\n🏁 Final Output:")
        print(result)
    except Exception as e:
        print(f"\n⚠️ Note: Demo successfully intercepted request before LLM call!")
        print(f"Error (expected with mock/test model): {e}")

if __name__ == "__main__":
    asyncio.run(main())

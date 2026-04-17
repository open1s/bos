import asyncio
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "brainos"))

from pybos import Agent, AgentConfig, HookEvent, PythonTool


def load_config():
    from pybos import ConfigLoader
    loader = ConfigLoader(strategy="deep_merge")
    loader.discover()
    cfg = loader.load_sync()
    agent_cfg = cfg.get("agent", {})
    global_cfg = cfg.get("global_model", {})
    return AgentConfig(
        name=agent_cfg.get("name", "assistant"),
        model=global_cfg.get("model", "gpt-4.1"),
        api_key=global_cfg.get("api_key", os.getenv("OPENAI_API_KEY", "")),
        base_url=global_cfg.get("base_url", os.getenv("OPENAI_BASE_URL", "https://api.openai.com/v1")),
        system_prompt=agent_cfg.get("system_prompt", "You are a helpful assistant."),
        temperature=agent_cfg.get("temperature", 0.7),
        timeout_secs=agent_cfg.get("timeout_secs", 120),
    )


def make_hooks_demo():
    print("=" * 60)
    print("DEMO: All Hook Events")
    print("=" * 60)
    
    config = load_config()
    agent = Agent.from_config(config)
    
    print("\n1. Adding a tool for react() testing...")
    def add_numbers(a: int, b: int) -> str:
        return str(a + b)
    
    tool = PythonTool(
        name="add",
        description="Add two numbers",
        parameters='{"a": {"type": "int"}, "b": {"type": "int"}}',
        schema='{"a": {"type": "int"}, "b": {"type": "int"}}',
        callback=add_numbers
    )
    agent.add_tool(tool)
    print("   Tool 'add' registered")
    
    print("\n2. Registering all 7 hook events...")
    hook_log = []
    
    def log_hook(event, ctx):
        hook_log.append(str(event))
        return "continue"
    
    for event_name in ["BeforeLlmCall", "AfterLlmCall", "BeforeToolCall", 
                       "AfterToolCall", "OnMessage", "OnComplete", "OnError"]:
        agent.register_hook(HookEvent(event_name), log_hook)
    
    print("   All 7 hooks registered: BeforeLlmCall, AfterLlmCall, BeforeToolCall, AfterToolCall, OnMessage, OnComplete, OnError")
    
    return agent, hook_log


async def test_run_simple(agent, hook_log):
    print("\n" + "=" * 60)
    print("TEST 1: run_simple() - LLM hooks only")
    print("=" * 60)
    
    hook_log.clear()
    print("\nRunning: agent.run_simple('What is 2+2?')")
    try:
        result = await agent.run_simple("What is 2+2?")
        print(f"Result: {result}")
    except Exception as e:
        print(f"Error: {e}")
    
    print(f"\nHooks fired: {len(hook_log)}")
    for h in hook_log:
        print(f"  - {h}")
    
    return hook_log


async def test_react_with_tool(agent, hook_log):
    print("\n" + "=" * 60)
    print("TEST 2: react() with tool call")
    print("=" * 60)
    
    hook_log.clear()
    print("\nRunning: agent.react('What is 5+3? Use the add tool.')")
    try:
        result = await agent.react("What is 5+3? Use the add tool.")
        print(f"Result: {result}")
    except Exception as e:
        print(f"Error: {e}")
    
    print(f"\nHooks fired: {len(hook_log)}")
    for h in hook_log:
        print(f"  - {h}")
    
    return hook_log


async def test_error_handling(hook_log):
    print("\n" + "=" * 60)
    print("TEST 3: Error hook (with bad API key)")
    print("=" * 60)
    
    hook_log.clear()
    print("\nRunning with invalid API key to trigger OnError...")
    config = load_config()
    bad_config = AgentConfig(
        name=config.name,
        model=config.model,
        api_key="invalid-key-trigger-error",
        base_url=config.base_url,
        system_prompt=config.system_prompt,
        temperature=config.temperature,
        timeout_secs=config.timeout_secs,
    )
    bad_agent = Agent.from_config(bad_config)
    
    def log_hook(event, ctx):
        hook_log.append(str(event))
        return "continue"
    
    for event_name in ["BeforeLlmCall", "AfterLlmCall", "BeforeToolCall", 
                       "AfterToolCall", "OnMessage", "OnComplete", "OnError"]:
        bad_agent.register_hook(HookEvent(event_name), log_hook)
    
    try:
        result = await bad_agent.run_simple("test")
        print(f"Result: {result}")
    except Exception as e:
        print(f"Error (expected): {type(e).__name__}")
    
    print(f"\nHooks fired: {len(hook_log)}")
    for h in hook_log:
        print(f"  - {h}")


async def main():
    print("PyBOS Hook & Plugin Full Demo")
    print("=" * 60)
    print(f"\nConfig: {load_config().model}")
    
    agent, hook_log = make_hooks_demo()
    
    await test_run_simple(agent, hook_log)
    await test_react_with_tool(agent, hook_log)
    await test_error_handling(hook_log)
    
    print("\n" + "=" * 60)
    print("SUMMARY")
    print("=" * 60)
    print("""
Hook Events:
  - BeforeLlmCall: Before sending request to LLM
  - AfterLlmCall: After receiving response from LLM
  - BeforeToolCall: Before executing a tool
  - AfterToolCall: After tool execution completes
  - OnMessage: When message is added to conversation
  - OnComplete: When agent completes successfully
  - OnError: When an error occurs

Hook Decisions:
  - 'continue': Proceed normally
  - 'abort': Stop current operation
  - 'error:message': Return error to caller
""")


if __name__ == "__main__":
    asyncio.run(main())
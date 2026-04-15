import asyncio
from pybos import Agent, AgentConfig, HookEvent

async def demo():
    print("=== PyBOS Agent Hook Demo ===\n")

    print("1. Creating agent...")
    config = AgentConfig(
        name="assistant",
        model="gpt-4",
        api_key="sk-test",
        base_url="https://api.openai.com/v1",
        system_prompt="You are a helpful assistant.",
        temperature=0.7,
        timeout_secs=120,
    )
    
    agent = Agent.from_config(config)
    print("   Agent created!\n")

    print("2. Registering hooks...")
    
    def before_tool_call(event, ctx):
        print(f"   [BeforeToolCall] {ctx.data.get('tool_name', 'unknown')}")
        return "continue"
    
    def after_tool_call(event, ctx):
        print(f"   [AfterToolCall] {ctx.data.get('tool_name', 'unknown')}")
        return "continue"
    
    def before_llm_call(event, ctx):
        print(f"   [BeforeLlmCall] Starting LLM call")
        return "continue"
    
    def after_llm_call(event, ctx):
        print(f"   [AfterLlmCall] LLM call completed")
        return "continue"
    
    def on_error(event, ctx):
        print(f"   [OnError] {ctx.data.get('error', 'unknown error')}")
        return "continue"

    agent.register_hook(HookEvent("BeforeToolCall"), before_tool_call)
    agent.register_hook(HookEvent("AfterToolCall"), after_tool_call)
    agent.register_hook(HookEvent("BeforeLlmCall"), before_llm_call)
    agent.register_hook(HookEvent("AfterLlmCall"), after_llm_call)
    agent.register_hook(HookEvent("OnError"), on_error)

    print("   Hooks registered!\n")

    print("3. Available hook events:")
    print("   - BeforeToolCall / AfterToolCall: around tool execution")
    print("   - BeforeLlmCall / AfterLlmCall: around LLM calls")
    print("   - OnMessage / OnComplete: message and completion events")
    print("   - OnError: when errors occur\n")

    print("4. Hook decisions:")
    print("   - 'continue' or return nothing: proceed normally")
    print("   - 'abort': abort the current operation")
    print("   - 'error:message': return an error\n")

    print("5. When agent.run_simple() or agent.react() is called,")
    print("   hooks will fire at the appropriate times.\n")

    print("=== Done ===")

asyncio.run(demo())

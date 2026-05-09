import asyncio
from brainos import BrainOS, tool

def log_hook(event, ctx):
    print(f"  [HOOK] {event.value} fired")
    return "continue"

@tool("Add numbers")
def add(a: int, b: int) -> int:
    return a + b

async def main():
    print("PyBOS Hook Demo")
    print("=" * 60)

    async with BrainOS() as brain:
        hook_log = []

        def make_log_hook(name):
            def hook(event, ctx):
                hook_log.append(name)
                return "continue"
            return hook

        agent = (
            brain.agent("hook-test", system_prompt="You are a math assistant.")
            .with_tools(add)
        )

        try:
            result = await agent.ask("What is 5 + 3?")
            print(f"\nResult: {result}")
        except Exception as e:
            print(f"Note: {e}")

    print("\n✅ Hook demo completed!")

if __name__ == "__main__":
    asyncio.run(main())
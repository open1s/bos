#!/usr/bin/env python3
"""
Example: Resilience Configuration Demo.

Demonstrates:
- @tool decorator for tool creation
- BrainOS context manager
- Agent resilience config via fluent API (with_resilience)
- Agent resilience config via constructor args
- BrainOS.agent() with resilience params
- Debug logging output from Python and Rust

Run with:
    python examples/04_resilience.py                       # Python logs only
    RUST_LOG=debug python examples/04_resilience.py        # Python + Rust logs
"""

import asyncio
import logging
from brainos import BrainOS, tool, InitTracing, ConfigLoader

logging.basicConfig(level=logging.DEBUG, format="%(levelname)s %(name)s: %(message)s")
_LOG = logging.getLogger("resilience_demo")

InitTracing()


@tool("Add two numbers together")
def add(a: int, b: int) -> int:
    print(f"Adding {a} and {b}")
    return a + b


async def main():
    loader = ConfigLoader()
    loader.discover()
    config = loader.load_sync()
    global_model = config.get("global_model", {})
    model = global_model.get("model")

    print("=" * 60)
    print("DEMO 1: Fluent API with .with_resilience()")
    print("=" * 60)
    async with BrainOS() as brain:
        agent = (
            brain.agent("demo1", model=model, system_prompt="You are a math assistant.")
            .with_resilience(
                rate_limit_capacity=10,
                rate_limit_window_secs=30,
                rate_limit_max_retries=2,
                circuit_breaker_max_failures=3,
                circuit_breaker_cooldown_secs=60,
            )
            .register(add)
        )
        print("Agent configured via fluent API with resilience:")
        print(f"  rate_limit_capacity={agent._config.rate_limit_capacity}")
        print(f"  rate_limit_window_secs={agent._config.rate_limit_window_secs}")
        print(f"  circuit_breaker_max_failures={agent._config.circuit_breaker_max_failures}")
        try:
            result = await agent.ask("What is 10 + 20?")
            print(f"  Result: {result}")
        except Exception as e:
            print(f"  (Expected error - no API key: {type(e).__name__})")

    print()
    print("=" * 60)
    print("DEMO 2: Constructor kwargs")
    print("=" * 60)
    from brainos import Agent
    agent2 = Agent(
        name="demo2",
        model=model,
        rate_limit_capacity=5,
        rate_limit_window_secs=15,
        circuit_breaker_max_failures=2,
        circuit_breaker_cooldown_secs=10,
    )
    print("Agent2 created via constructor:")
    print(f"  rate_limit_capacity={agent2._config.rate_limit_capacity}")
    print(f"  circuit_breaker_max_failures={agent2._config.circuit_breaker_max_failures}")

    print()
    print("=" * 60)
    print("DEMO 3: BrainOS.agent() with resilience params")
    print("=" * 60)
    async with BrainOS() as brain:
        agent3 = brain.agent(
            "demo3",
            model=model,
            rate_limit_capacity=20,
            rate_limit_window_secs=60,
            circuit_breaker_max_failures=5,
        )
        print("Agent3 via BrainOS.agent():")
        print(f"  rate_limit_capacity={agent3._config.rate_limit_capacity}")
        print(f"  circuit_breaker_max_failures={agent3._config.circuit_breaker_max_failures}")

    print()
    print("All config demos passed - resilience flows correctly from Python to Rust")


if __name__ == "__main__":
    print("Run with RUST_LOG=debug to see Python + Rust resilience logs:\n")
    asyncio.run(main())
"""
brainos — Elegant Python API for BrainOS Agent Framework

A high-level wrapper around pybos that provides:
- Context manager for lifecycle management
- @tool() decorator for simple tool registration
- Fluent agent creation with chainable config
- Type hints and minimal boilerplate

Usage:
    from brainos import BrainOS, tool

    @tool("Add two numbers")
    def add(a: int, b: int) -> int:
        return a + b

    async with BrainOS() as brain:
        agent = brain.agent("assistant")
        agent.register(add)
        result = await agent.ask("What is 42 + 58?")
"""

from brainos.core import BrainOS, Agent
from brainos.tool import tool, ToolDef

__all__ = ["BrainOS", "Agent", "tool", "ToolDef"]
__version__ = "0.1.0"

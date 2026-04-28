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
from brainos.bus import BusManager, Publisher, Subscriber
from brainos.query import Query, Queryable
from brainos.caller import Caller, Callable
from brainos.config import Config
from pybos import AgentConfig, AgentPlugin, PluginRegistry, ConfigLoader, init_tracing as InitTracing

__all__ = [
    "BrainOS", "Agent",
    "tool", "ToolDef",
    "BusManager", "Publisher", "Subscriber",
    "Query", "Queryable",
    "Caller", "Callable",
    "Config",
    "AgentConfig",
    "AgentPlugin",
    "PluginRegistry",
    "ConfigLoader",
    "InitTracing",
]
__version__ = "1.2.0"

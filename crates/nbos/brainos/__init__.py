"""
brainos - Elegant Python API for BrainOS Agent Framework

Usage:
    from brainos import BrainOS, tool

    @tool("Add two numbers")
    def add(a: int, b: int) -> int:
        return a + b

    async with BrainOS() as brain:
        agent = (
            brain.agent("assistant")
            .with_tools(add)
            .with_prompt("You are a helpful math assistant.")
        )
        result = await agent.ask("What is 2+2?")
"""

from brainos.core import (
    BrainOS,
    Agent,
    AgentBuilder,
    ToolRegistry,
    SessionManager,
)
from brainos.tool import tool, ToolDef, ToolResult
from brainos.bus import BusManager, Publisher, Subscriber
from brainos.query import Query, Queryable
from brainos.caller import Caller, Callable
from brainos.config import Config
from nbos import (
    AgentConfig,
    AgentPlugin,
    PluginRegistry,
    ConfigLoader,
    HookEvent,
    HookDecision,
    HookContext,
    init_tracing as InitTracing,
)

__all__ = [
    "BrainOS",
    "Agent",
    "AgentBuilder",
    "tool",
    "ToolDef",
    "ToolResult",
    "ToolRegistry",
    "SessionManager",
    "BusManager",
    "Publisher",
    "Subscriber",
    "Query",
    "Queryable",
    "Caller",
    "Callable",
    "AgentConfig",
    "AgentPlugin",
    "PluginRegistry",
    "ConfigLoader",
    "HookEvent",
    "HookDecision",
    "HookContext",
    "InitTracing",
]
__version__ = "2.1.0"
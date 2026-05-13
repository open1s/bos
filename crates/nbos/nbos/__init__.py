"""nbos - Python API for BrainOS Agent Framework

Usage:
    from nbos import BrainOS, tool

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

from nbos.core import (
    BrainOS,
    Agent,
    AgentBuilder,
    ToolRegistry,
    SessionManager,
)
from nbos.tool import tool, ToolDef, ToolResult
from nbos.bus import BusManager, Publisher, Subscriber
from nbos.query import Query, Queryable
from nbos.caller import Caller, Callable
from nbos.config import Config
from nbos_native import (
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

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
    Agent as PyAgent,
    AgentCallableServer,
    AgentConfig,
    AgentPlugin,
    AgentRpcClient,
    BudgetStatus,
    Bus,
    BusConfig,
    Callable,
    Caller,
    ConfigLoader,
    HookEvent,
    HookDecision,
    HookContext,
    HookRegistry,
    LlmMessage,
    LlmRequestWrapper,
    LlmResponseWrapper,
    LlmUsage,
    McpClient,
    PluginRegistry,
    PromptTokensDetails,
    Publisher,
    PythonTool,
    Query,
    Queryable,
    QueryStreamIterator,
    StreamSender,
    Subscriber,
    TokenBudgetReport,
    TokenUsage,
    ToolCallWrapper,
    ToolResultWrapper,
    init_tracing as InitTracing,
)

# Export both casing styles for backward compatibility
init_tracing = InitTracing

__all__ = [
    "BrainOS",
    "Agent",
    "PyAgent",
    "AgentBuilder",
    "AgentCallableServer",
    "AgentConfig",
    "AgentPlugin",
    "AgentRpcClient",
    "tool",
    "ToolDef",
    "ToolResult",
    "ToolRegistry",
    "SessionManager",
    "BusManager",
    "Bus",
    "BusConfig",
    "Publisher",
    "Subscriber",
    "Query",
    "Queryable",
    "QueryStreamIterator",
    "StreamSender",
    "Caller",
    "Callable",
    "Caller",
    "Callable",
    "AgentConfig",
    "AgentPlugin",
    "PluginRegistry",
    "ConfigLoader",
    "Config",
    "HookEvent",
    "HookDecision",
    "HookContext",
    "HookRegistry",
    "LlmMessage",
    "LlmRequestWrapper",
    "LlmResponseWrapper",
    "LlmUsage",
    "McpClient",
    "PythonTool",
    "TokenBudgetReport",
    "TokenUsage",
    "PromptTokensDetails",
    "ToolCallWrapper",
    "ToolResultWrapper",
    "BudgetStatus",
    "InitTracing",
    "init_tracing",
]
__version__ = "2.1.1"

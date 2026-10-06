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
from nbos.bus import (
    BusManager,
    Publisher as PublisherWrapper,
    Subscriber as SubscriberWrapper,
)
from nbos.query import Query as QueryWrapper, Queryable as QueryableWrapper
from nbos.caller import Caller as CallerWrapper, Callable as CallableWrapper
from nbos.config import Config
from nbos.content import Content, ContentPart, Binary
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

# The native (pyo3) bus classes keep their plain exported names; the high
# level wrappers returned by BusManager are available under the *_Wrapper aliases.
PyPublisher = Publisher
PySubscriber = Subscriber
PyQuery = Query
PyQueryable = Queryable
PyCaller = Caller
PyCallable = Callable

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
    "PublisherWrapper",
    "SubscriberWrapper",
    "QueryWrapper",
    "QueryableWrapper",
    "CallerWrapper",
    "CallableWrapper",
    "PyPublisher",
    "PySubscriber",
    "PyQuery",
    "PyQueryable",
    "PyCaller",
    "PyCallable",
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
    "Content",
    "ContentPart",
    "Binary",
]
# Keep in sync with pyproject.toml; tests/test_public_api.py enforces this.
__version__ = "2.4.2"

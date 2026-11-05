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

from typing import Any

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
from nbos.memory import Memory
from nbos_native import (
    Agent as PyAgent,
    AgentCallableServer,
    AgentConfig,
    AgentPlugin,
    AgentRpcClient,
    BudgetStatus,
    FallbackProvider,
    FerrisHarness,
    FerrisManifest,
    FerrisManifestBuilder,
    Goal,
    GoalStore,
    JobRegistry,
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
    estimate_tokens,
    init_tracing as InitTracing,
    render_template,
    with_memory,
)

# FerrisHost is only present when the native module was built with the
# ferris-host feature (the default). Keep the authoring-only build importable.
try:
    from nbos_native import FerrisHost as _NativeFerrisHost
except ImportError:  # pragma: no cover - authoring-only native build

    class FerrisHost:  # type: ignore[no-redef]
        """Unavailable in a native build compiled without ferris-host."""

        def __init__(self, *args: Any, **kwargs: Any) -> None:
            raise RuntimeError(
                "FerrisHost is not built; rebuild nbos with the ferris-host feature"
            )

else:

    class FerrisHost:  # type: ignore[no-redef]
        """Ferris host with Python-side agent integration helpers."""

        def __init__(self, *args: Any, **kwargs: Any) -> None:
            self._inner = _NativeFerrisHost(*args, **kwargs)

        def __getattr__(self, name: str) -> Any:
            return getattr(self._inner, name)

        def __repr__(self) -> str:
            return repr(self._inner)

        async def register_agent_tools(self, agent: Any) -> list[dict[str, str]]:
            """Register every applied tool contribution on an Agent/AgentBuilder."""
            import json

            from nbos.tool import ToolDef

            registered: list[dict[str, str]] = []
            tools = getattr(agent, "_tools", None)
            inner = getattr(agent, "_inner", None)
            for extension, entries in self._inner.contributions().items():
                for contribution in entries:
                    if contribution.get("kind") != "tool":
                        continue
                    payload = contribution.get("payload") or {}
                    schema = payload.get("input_schema") or {}
                    name = contribution["id"]

                    async def call(args: dict, _extension=extension, _name=name) -> Any:
                        return await self._inner.invoke(
                            _extension, "tool", _name, args
                        )

                    td = ToolDef(
                        name=name,
                        description=payload.get("description") or name,
                        callback=call,
                        parameters=schema.get("properties", {}),
                        schema=schema,
                    )
                    if tools is not None and hasattr(tools, "add"):
                        tools.add(td)
                    if inner is not None and hasattr(inner, "add_tool"):
                        from nbos_native import PythonTool

                        await inner.add_tool(
                            PythonTool(
                                name=td.name,
                                description=td.description,
                                parameters=json.dumps(td.parameters),
                                schema=json.dumps(td.schema),
                                callback=td.callback,
                            )
                        )
                    registered.append({"extension": extension, "name": name})
            return registered


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
    "Memory",
    "Binary",
    "Goal",
    "GoalStore",
    "JobRegistry",
    "FallbackProvider",
    "FerrisHarness",
    "FerrisHost",
    "FerrisManifest",
    "FerrisManifestBuilder",
    "render_template",
    "with_memory",
    "estimate_tokens",
]
# Keep in sync with pyproject.toml; tests/test_public_api.py enforces this.
__version__ = "4.0.2"

"""Core BrainOS wrapper — lifecycle management and agent creation.

Elegant, chainable API for building AI agents with tools.

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

from __future__ import annotations

import json
import logging
from typing import Any, TYPE_CHECKING
from contextlib import AbstractAsyncContextManager

if TYPE_CHECKING:
    pass

logging.basicConfig(level=logging.DEBUG, format='%(levelname)s %(name)s: %(message)s')
_LOG = logging.getLogger("brainos.core")

from pybos import Agent as PyAgent
from pybos import AgentConfig as PyAgentConfig
from pybos import Bus as PyBus
from pybos import BusConfig as PyBusConfig
from pybos import ConfigLoader as PyConfigLoader
from pybos import PythonTool

from brainos.tool import ToolDef


class Agent:
    """High-level agent wrapper with fluent, chainable API.

    All configuration methods return `self` for chaining:

        agent = (
            brain.agent("assistant")
            .with_model("gpt-4")
            .with_tools(add, multiply)
            .with_temperature(0.5)
        )

    Auto-starts on first `ask()` — or call `.start()` explicitly.
    """

    def __init__(
        self,
        bus: PyBus | None,
        *,
        name: str = "assistant",
        model: str | None = None,
        base_url: str | None = None,
        api_key: str | None = None,
        system_prompt: str = "You are a helpful assistant.",
        temperature: float = 0.7,
        max_tokens: int | None = None,
        timeout_secs: int = 120,
        # Resilience config
        rate_limit_capacity: int | None = None,
        rate_limit_window_secs: int | None = None,
        rate_limit_max_retries: int | None = None,
        rate_limit_retry_backoff_secs: int | None = None,
        rate_limit_auto_wait: bool | None = None,
        circuit_breaker_max_failures: int | None = None,
        circuit_breaker_cooldown_secs: int | None = None,
    ) -> None:
        self._bus = bus
        self._tools: list[ToolDef] = []
        self._agent: PyAgent | None = None
        self._started = False

        # Build config with defaults from environment/config file
        loader = PyConfigLoader()
        loader.discover()
        config = loader.load_sync()
        global_model = config.get("global_model", {})

        self._config = PyAgentConfig()
        self._config.name = name
        self._config.model = model or global_model.get("model", "nvidia/meta/llama-3.1-8b-instruct")
        self._config.base_url = base_url or global_model.get("base_url", "https://integrate.api.nvidia.com/v1")
        self._config.api_key = api_key or global_model.get("api_key") or ""
        self._config.system_prompt = system_prompt
        self._config.temperature = temperature
        if max_tokens is not None:
            self._config.max_tokens = max_tokens
        self._config.timeout_secs = timeout_secs

        self._apply_resilience(
            rate_limit_capacity, rate_limit_window_secs, rate_limit_max_retries,
            rate_limit_retry_backoff_secs, rate_limit_auto_wait,
            circuit_breaker_max_failures, circuit_breaker_cooldown_secs,
        )

    def _apply_resilience(self, rate_limit_capacity, rate_limit_window_secs, rate_limit_max_retries, rate_limit_retry_backoff_secs, rate_limit_auto_wait, circuit_breaker_max_failures, circuit_breaker_cooldown_secs):
        if rate_limit_capacity is not None:
            self._config.rate_limit_capacity = rate_limit_capacity
        if rate_limit_window_secs is not None:
            self._config.rate_limit_window_secs = rate_limit_window_secs
        if rate_limit_max_retries is not None:
            self._config.rate_limit_max_retries = rate_limit_max_retries
        if rate_limit_retry_backoff_secs is not None:
            self._config.rate_limit_retry_backoff_secs = rate_limit_retry_backoff_secs
        if rate_limit_auto_wait is not None:
            self._config.rate_limit_auto_wait = rate_limit_auto_wait
        if circuit_breaker_max_failures is not None:
            self._config.circuit_breaker_max_failures = circuit_breaker_max_failures
        if circuit_breaker_cooldown_secs is not None:
            self._config.circuit_breaker_cooldown_secs = circuit_breaker_cooldown_secs

    # ── Fluent config ──────────────────────────────────────────────────────────

    def with_model(self, model: str) -> "Agent":
        """Set the LLM model. E.g. 'gpt-4', 'nvidia/llama-3.1-nemotron-70b'."""
        self._config.model = model
        return self

    def with_prompt(self, prompt: str) -> "Agent":
        """Set the system prompt."""
        self._config.system_prompt = prompt
        return self

    def with_temperature(self, temperature: float) -> "Agent":
        """Set sampling temperature (0.0–2.0)."""
        self._config.temperature = temperature
        return self

    def with_max_tokens(self, max_tokens: int) -> "Agent":
        """Set max tokens in response."""
        self._config.max_tokens = max_tokens
        return self

    def with_timeout(self, secs: int) -> "Agent":
        """Set request timeout in seconds."""
        self._config.timeout_secs = secs
        return self

    def with_resilience(
        self,
        rate_limit_capacity: int = 40,
        rate_limit_window_secs: int = 60,
        rate_limit_max_retries: int = 3,
        rate_limit_retry_backoff_secs: int = 1,
        rate_limit_auto_wait: bool = True,
        circuit_breaker_max_failures: int = 5,
        circuit_breaker_cooldown_secs: int = 30,
    ) -> "Agent":
        """Configure rate limiting and circuit breaker."""
        self._config.rate_limit_capacity = rate_limit_capacity
        self._config.rate_limit_window_secs = rate_limit_window_secs
        self._config.rate_limit_max_retries = rate_limit_max_retries
        self._config.rate_limit_retry_backoff_secs = rate_limit_retry_backoff_secs
        self._config.rate_limit_auto_wait = rate_limit_auto_wait
        self._config.circuit_breaker_max_failures = circuit_breaker_max_failures
        self._config.circuit_breaker_cooldown_secs = circuit_breaker_cooldown_secs
        return self

    # ── Tool registration ───────────────────────────────────────────────────────

    def with_tools(self, *tools: ToolDef) -> "Agent":
        """Register one or more tools. Chainable.

        Usage:
            agent.with_tools(add)
            agent.with_tools(add, multiply, weather)
        """
        for t in tools:
            self._tools.append(t)
        return self

    def register(self, tool_def: ToolDef) -> "Agent":
        """Register a single tool (alias for with_tools for compatibility)."""
        self._tools.append(tool_def)
        return self

    def register_many(self, *tools: ToolDef) -> "Agent":
        """Register multiple tools at once (alias for with_tools)."""
        return self.with_tools(*tools)

    # ── Skills ─────────────────────────────────────────────────────────────────

    def with_skills(self, dir_path: str) -> "Agent":
        """Load skills from a directory."""
        self._skills_dir = dir_path
        return self

    # ── Lifecycle ──────────────────────────────────────────────────────────────

    async def start(self) -> "Agent":
        """Eagerly initialize the agent. Optional — auto-starts on first ask()."""
        if self._started:
            return self
        self._agent = await PyAgent.create(self._config, self._bus or PyBus.create(PyBusConfig()))
        for t in self._tools:
            py_tool = PythonTool(
                name=t.name,
                description=t.description,
                parameters=json.dumps(t.parameters),
                schema=json.dumps(t.schema),
                callback=t.callback,
            )
            await self._agent.add_tool(py_tool)
        if hasattr(self, '_skills_dir') and self._skills_dir:
            await self._agent.register_skills_from_dir(self._skills_dir)
        self._started = True
        return self

    # ── Interaction ─────────────────────────────────────────────────────────────

    async def ask(self, question: str) -> str:
        """Ask the agent a question. Auto-starts if needed."""
        if not self._started:
            await self.start()
        return await self._agent.run_simple(question)

    async def chat(self, message: str) -> str:
        """Send a message (alias for ask)."""
        return await self.ask(message)

    async def run_simple(self, message: str) -> str:
        """Run a simple conversation (single LLM call with tools)."""
        return await self.ask(message)

    async def react(self, task: str) -> str:
        """Run the agent with ReAct reasoning loop (tool use)."""
        if not self._started:
            await self.start()
        return await self._agent.react(task)

    async def stream(self, task: str):
        """Stream tokens as they are generated.

        Usage:
            async for chunk in await agent.stream("hello"):
                print(chunk, end="", flush=True)
        """
        if not self._started:
            await self.start()
        return await self._agent.stream(task)

    # ── Introspection ───────────────────────────────────────────────────────────

    @property
    def tools(self) -> list[str]:
        """List registered tool names."""
        if self._agent is None:
            return [t.name for t in self._tools]
        return self._agent.list_tools()

    @property
    def config(self) -> dict[str, Any]:
        """Get agent config as dict."""
        return {
            "name": self._config.name,
            "model": self._config.model,
            "base_url": self._config.base_url,
        }


class BrainOS(AbstractAsyncContextManager):
    """Main entry point — manages Bus lifecycle and agent creation.

    Auto-discovers config from ~/.bos/conf/config.toml and environment variables.

    Usage:
        async with BrainOS() as brain:
            agent = brain.agent("assistant")
            result = await agent.ask("What is 2+2?")
    """

    def __init__(
        self,
        *,
        api_key: str | None = None,
        base_url: str | None = None,
        model: str | None = None,
    ) -> None:
        # Load global config
        loader = PyConfigLoader()
        loader.discover()
        config = loader.load_sync()
        global_model = config.get("global_model", {})

        self._api_key = api_key or global_model.get("api_key")
        self._base_url = base_url or global_model.get("base_url", "https://integrate.api.nvidia.com/v1")
        self._model = model or global_model.get("model", "nvidia/meta/llama-3.1-8b-instruct")
        self._bus: PyBus | None = None

    async def __aenter__(self) -> "BrainOS":
        self._bus = await PyBus.create(PyBusConfig())
        return self

    async def __aexit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        pass

    def agent(
        self,
        name: str = "assistant",
        *,
        tools: list[ToolDef] | None = None,
        system_prompt: str = "You are a helpful assistant.",
        model: str | None = None,
        temperature: float = 0.7,
        timeout_secs: int = 120,
    ) -> Agent:
        """Create a new agent with the given configuration.

        Usage:
            # Minimal
            agent = brain.agent("assistant")

            # With tools
            agent = brain.agent("math-bot", tools=[add, multiply])

            # With full config
            agent = (
                brain.agent("assistant", model="gpt-4", temperature=0.5)
                .with_tools(add, multiply)
                .with_prompt("You are a math expert.")
            )
        """
        agent = Agent(
            bus=self._bus,
            name=name,
            model=model or self._model,
            base_url=self._base_url,
            api_key=self._api_key,
            system_prompt=system_prompt,
            temperature=temperature,
            timeout_secs=timeout_secs,
        )
        if tools:
            agent.with_tools(*tools)
        return agent

    @property
    def bus(self) -> PyBus:
        if self._bus is None:
            raise RuntimeError("BrainOS not started. Use 'async with' context.")
        return self._bus
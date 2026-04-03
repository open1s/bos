"""Core BrainOS wrapper — lifecycle management and agent creation."""

from __future__ import annotations

import json
import os
from typing import Any
from contextlib import AbstractAsyncContextManager

from pybos import Agent as PyAgent
from pybos import AgentConfig as PyAgentConfig
from pybos import Bus as PyBus
from pybos import BusConfig as PyBusConfig
from pybos import ConfigLoader as PyConfigLoader
from pybos import PythonTool

from brainos.tool import ToolDef


class Agent:
    """High-level agent wrapper with fluent API.

    Usage:
        agent = Agent(bus, name="assistant")
        agent.register(my_tool)
        result = await agent.ask("What is 2+2?")
    """

    def __init__(
        self,
        bus: PyBus,
        *,
        name: str = "assistant",
        model: str = "nvidia/meta/llama-3.1-8b-instruct",
        base_url: str = "https://integrate.api.nvidia.com/v1",
        api_key: str | None = None,
        system_prompt: str = "You are a helpful assistant.",
        temperature: float = 0.7,
        max_tokens: int | None = None,
        timeout_secs: int = 120,
    ) -> None:
        self._bus = bus
        self._tools: list[ToolDef] = []
        self._agent: PyAgent | None = None

        self._config = PyAgentConfig()
        self._config.name = name
        self._config.model = model
        self._config.base_url = base_url
        self._config.api_key = api_key
        self._config.system_prompt = system_prompt
        self._config.temperature = temperature
        if max_tokens is not None:
            self._config.max_tokens = max_tokens
        self._config.timeout_secs = timeout_secs

    # ── Fluent config ──────────────────────────────────────────────

    def with_model(self, model: str) -> Agent:
        self._config.model = model
        return self

    def with_prompt(self, prompt: str) -> Agent:
        self._config.system_prompt = prompt
        return self

    def with_temperature(self, temp: float) -> Agent:
        self._config.temperature = temp
        return self

    def with_timeout(self, secs: int) -> Agent:
        self._config.timeout_secs = secs
        return self

    # ── Tool registration ──────────────────────────────────────────

    def register(self, tool_def: ToolDef) -> Agent:
        """Register a tool created with @tool()."""
        self._tools.append(tool_def)
        return self

    def register_many(self, *tools: ToolDef) -> Agent:
        """Register multiple tools at once."""
        for t in tools:
            self._tools.append(t)
        return self

    # ── Skills ─────────────────────────────────────────────────────

    def register_skills(self, dir_path: str) -> Agent:
        self._skills_dir = dir_path
        return self

    # ── Lifecycle ──────────────────────────────────────────────────

    async def start(self) -> Agent:
        """Build the underlying pybos agent and register tools."""
        self._agent = await PyAgent.create(self._config, self._bus)
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
        return self

    # ── Interaction ────────────────────────────────────────────────

    async def ask(self, question: str) -> str:
        """Ask the agent a question. Uses the full agent run_loop with tools."""
        if self._agent is None:
            await self.start()
        return await self._agent.run_simple(question)

    async def chat(self, message: str) -> str:
        """Send a message using simple conversation (no tool reasoning)."""
        if self._agent is None:
            await self.start()
        return await self._agent.run_simple(message)

    async def run_simple(self, message: str) -> str:
        """Run a simple conversation (no tool reasoning)."""
        if self._agent is None:
            await self.start()
        return await self._agent.run_simple(message)

    async def stream(self, task: str):
        """Stream tokens as they are generated.

        Returns an async iterator yielding text chunks.
        Usage:
            async for chunk in await agent.stream("hello"):
                print(chunk, end="", flush=True)
        """
        if self._agent is None:
            await self.start()
        return await self._agent.stream(task)

    async def react(self, task: str) -> str:
        """Run the agent with ReAct reasoning (tool use)."""
        if self._agent is None:
            await self.start()
        return await self._agent.react(task)

    @property
    def tools(self) -> list[str]:
        """List registered tool names."""
        if self._agent is None:
            return [t.name for t in self._tools]
        return self._agent.list_tools()

    @property
    def config(self) -> dict[str, Any]:
        """Get agent config as dict."""
        if self._agent is None:
            return {
                "name": self._config.name,
                "model": self._config.model,
                "base_url": self._config.base_url,
            }
        return self._agent.config()


class BrainOS(AbstractAsyncContextManager):
    """Main entry point — manages Bus lifecycle and agent creation.

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
        system_prompt: str = "You are a helpful assistant.",
        model: str | None = None,
        temperature: float = 0.7,
        timeout_secs: int = 120,
    ) -> Agent:
        """Create a new agent with the given configuration."""
        return Agent(
            bus=self._bus,
            name=name,
            model=model or self._model,
            base_url=self._base_url,
            api_key=self._api_key,
            system_prompt=system_prompt,
            temperature=temperature,
            timeout_secs=timeout_secs,
        )

    @property
    def bus(self) -> PyBus:
        if self._bus is None:
            raise RuntimeError("BrainOS not started. Use 'async with' context.")
        return self._bus

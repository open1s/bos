"""Fluent builder API for nbos Agent Framework.

Based on jsbos patterns with fluent builder API.
"""

from __future__ import annotations

import json
import logging
from typing import Any, Callable
from contextlib import AbstractAsyncContextManager

logging.basicConfig(level=logging.DEBUG, format='%(levelname)s %(name)s: %(message)s')
_LOG = logging.getLogger("nbos.core")

from nbos_native import Agent as PyAgent
from nbos_native import AgentConfig as PyAgentConfig
from nbos_native import AgentPlugin as PyAgentPlugin
from nbos_native import Bus as PyBus
from nbos_native import BusConfig as PyBusConfig
from nbos_native import ConfigLoader as PyConfigLoader
from nbos_native import PythonTool
from nbos_native import HookEvent, HookDecision, HookContext

from nbos.tool import ToolDef


DEFAULT_MODEL = "nvidia/meta/llama-3.1-8b-instruct"
DEFAULT_BASE_URL = "https://integrate.api.nvidia.com/v1"


class ToolRegistry:
    """Registry for managing multiple tools."""

    def __init__(self, tools: list[ToolDef] | None = None) -> None:
        self._tools: dict[str, ToolDef] = {}
        if tools:
            for t in tools:
                self.add(t)

    def add(self, tool: ToolDef) -> "ToolRegistry":
        self._tools[tool.name] = tool
        return self

    def register(self, tool: ToolDef) -> "ToolRegistry":
        return self.add(tool)

    def remove(self, name: str) -> "ToolRegistry":
        self._tools.pop(name, None)
        return self

    def get(self, name: str) -> ToolDef | None:
        return self._tools.get(name)

    def has(self, name: str) -> bool:
        return name in self._tools

    def list(self) -> list[str]:
        return list(self._tools.keys())

    def list_tools(self) -> list[ToolDef]:
        return list(self._tools.values())

    def size(self) -> int:
        return len(self._tools)

    def clear(self) -> "ToolRegistry":
        self._tools.clear()
        return self

    def merge(self, other: "ToolRegistry") -> "ToolRegistry":
        for t in other.list_tools():
            self.add(t)
        return self


class SessionManager:
    """Session management for an agent."""

    def __init__(self, agent: PyAgent) -> None:
        self._agent = agent

    def save(self, path: str) -> "SessionManager":
        self._agent.save_message_log(path)
        return self

    def restore(self, path: str) -> "SessionManager":
        self._agent.restore_message_log(path)
        return self

    def save_full(self, path: str) -> "SessionManager":
        self._agent.save_session(path)
        return self

    def compact(self, keep_recent: int = 10, max_summary_chars: int = 2000) -> "SessionManager":
        self._agent.compact_message_log()
        return self

    def clear(self) -> "SessionManager":
        self._agent.clear_session_context()
        return self

    def get_messages(self) -> list[dict]:
        return self._agent.get_messages()

    def add_message(self, role: str, content: str) -> "SessionManager":
        self._agent.add_message({"role": role, "content": content})
        return self

    def export(self) -> dict:
        return self._agent.session_state()

    @property
    def context(self) -> dict:
        return self._agent.session_context()


class AgentBuilder:
    """Fluent builder for creating Agents with chainable configuration.

    Usage:
        agent = (
            AgentBuilder(brain.bus)
            .name("assistant")
            .with_tools(add, multiply)
            .with_prompt("You are a math expert.")
            .with_temperature(0.5)
            .with_hooks({"BeforeToolCall": my_hook})
            .start()
        )
    """

    def __init__(self, bus: PyBus | None, options: dict[str, Any] | None = None) -> None:
        self._bus = bus
        self._inner: PyAgent | None = None
        self._tools = ToolRegistry()
        self._hooks: list[tuple[str, Callable]] = []
        self._plugins: list[dict] = []
        self._skills: list[dict] = []
        self._mcp_servers: list[dict] = []

        opts = options or {}
        self._config = PyAgentConfig()
        self._config.name = opts.get("name", "assistant")
        self._config.model = opts.get("model", DEFAULT_MODEL)
        self._config.base_url = opts.get("base_url", DEFAULT_BASE_URL)
        self._config.api_key = opts.get("api_key", "")
        self._config.system_prompt = opts.get("system_prompt", "You are a helpful assistant.")
        self._config.temperature = opts.get("temperature", 0.7)
        self._config.timeout_secs = opts.get("timeout_secs", 120)
        if "max_tokens" in opts and opts["max_tokens"] is not None:
            self._config.max_tokens = opts["max_tokens"]

        if opts.get("rate_limit_capacity"):
            self._config.rate_limit_capacity = opts["rate_limit_capacity"]
        if opts.get("rate_limit_window_secs"):
            self._config.rate_limit_window_secs = opts["rate_limit_window_secs"]
        if opts.get("rate_limit_max_retries"):
            self._config.rate_limit_max_retries = opts["rate_limit_max_retries"]
        if opts.get("circuit_breaker_max_failures"):
            self._config.circuit_breaker_max_failures = opts["circuit_breaker_max_failures"]
        if opts.get("circuit_breaker_cooldown_secs"):
            self._config.circuit_breaker_cooldown_secs = opts["circuit_breaker_cooldown_secs"]

    def name(self, name: str) -> "AgentBuilder":
        self._config.name = name
        return self

    def with_model(self, model: str) -> "AgentBuilder":
        self._config.model = model
        return self

    def with_base_url(self, url: str) -> "AgentBuilder":
        self._config.base_url = url
        return self

    def with_api_key(self, key: str) -> "AgentBuilder":
        self._config.api_key = key
        return self

    def with_prompt(self, prompt: str) -> "AgentBuilder":
        self._config.system_prompt = prompt
        return self

    def with_temperature(self, temperature: float) -> "AgentBuilder":
        self._config.temperature = temperature
        return self

    def with_max_tokens(self, max_tokens: int) -> "AgentBuilder":
        self._config.max_tokens = max_tokens
        return self

    def with_timeout(self, secs: int) -> "AgentBuilder":
        self._config.timeout_secs = secs
        return self

    def with_tools(self, *tools: ToolDef) -> "AgentBuilder":
        for t in tools:
            if isinstance(t, ToolRegistry):
                self._tools.merge(t)
            else:
                self._tools.add(t)
        return self

    def register(self, *tools: ToolDef) -> "AgentBuilder":
        return self.with_tools(*tools)

    def with_resilience(
        self,
        rate_limit_capacity: int = 40,
        rate_limit_window_secs: int = 60,
        rate_limit_max_retries: int = 3,
        circuit_breaker_max_failures: int = 5,
        circuit_breaker_cooldown_secs: int = 30,
    ) -> "AgentBuilder":
        self._config.rate_limit_capacity = rate_limit_capacity
        self._config.rate_limit_window_secs = rate_limit_window_secs
        self._config.rate_limit_max_retries = rate_limit_max_retries
        self._config.circuit_breaker_max_failures = circuit_breaker_max_failures
        self._config.circuit_breaker_cooldown_secs = circuit_breaker_cooldown_secs
        return self

    def with_hooks(self, hooks: dict[str, Callable] | list[tuple[str, Callable]]) -> "AgentBuilder":
        if isinstance(hooks, dict):
            for event, callback in hooks.items():
                self._hooks.append((event, callback))
        else:
            for event, callback in hooks:
                self._hooks.append((event, callback))
        return self

    def hook(self, event: str, callback: Callable) -> "AgentBuilder":
        self._hooks.append((event, callback))
        return self

    def with_plugins(self, *plugins: dict) -> "AgentBuilder":
        self._plugins.extend(plugins)
        return self

    def plugin(self, name_or_obj: str | dict, **handlers) -> "AgentBuilder":
        if isinstance(name_or_obj, str):
            self._plugins.append({"name": name_or_obj, **handlers})
        else:
            self._plugins.append(name_or_obj)
        return self

    def with_skills_dir(self, dir_path: str) -> "AgentBuilder":
        self._skills.append({"dir_path": dir_path})
        return self

    def skill(self, name: str, content: str) -> "AgentBuilder":
        self._skills.append({"name": name, "content": content})
        return self

    def with_mcp(self, namespace: str, command: str, args: list[str]) -> "AgentBuilder":
        self._mcp_servers.append({"namespace": namespace, "command": command, "args": args, "type": "process"})
        return self

    def with_mcp_http(self, namespace: str, url: str) -> "AgentBuilder":
        self._mcp_servers.append({"namespace": namespace, "url": url, "type": "http"})
        return self

    def with_bash(self, name: str = "bash", workspace_root: str | None = None) -> "AgentBuilder":
        self._config._bash_tool = {"name": name, "workspace_root": workspace_root}
        return self

    async def start(self) -> "Agent":
        if self._bus is None:
            self._bus = await PyBus.create(PyBusConfig())

        self._inner = await PyAgent.create(self._config, self._bus)

        for td in self._tools.list_tools():
            py_tool = PythonTool(
                name=td.name,
                description=td.description,
                parameters=json.dumps(td.parameters),
                schema=json.dumps(td.schema),
                callback=td.callback,
            )
            await self._inner.add_tool(py_tool)

        if hasattr(self._config, "_bash_tool"):
            bash_cfg = self._config._bash_tool
            await self._inner.add_bash_tool(bash_cfg["name"], bash_cfg.get("workspace_root"))

        for event_name, callback in self._hooks:
            self._inner.register_hook(HookEvent(event_name), callback)

        for p in self._plugins:
            plugin_obj = PyAgentPlugin(
                name=p.get("name", "plugin"),
                on_llm_request=p.get("on_llm_request"),
                on_llm_response=p.get("on_llm_response"),
                on_tool_call=p.get("on_tool_call"),
                on_tool_result=p.get("on_tool_result"),
            )
            self._inner.register_plugin(plugin_obj)

        for s in self._skills:
            if "dir_path" in s:
                await self._inner.register_skills_from_dir(s["dir_path"])

        for m in self._mcp_servers:
            if m["type"] == "process":
                await self._inner.add_mcp_server(m["namespace"], m["command"], m["args"])
            else:
                await self._inner.add_mcp_server_http(m["namespace"], m["url"])

        return Agent(self._inner, self._tools)

    async def ask(self, prompt: str) -> str:
        if not self._inner:
            await self.start()
        return await self._inner.run_simple(prompt)

    async def chat(self, message: str) -> str:
        return await self.ask(message)

    async def react(self, task: str) -> str:
        if not self._inner:
            await self.start()
        return await self._inner.react(task)

    async def stream(self, task: str):
        if not self._inner:
            await self.start()
        return await self._inner.stream(task)


class Agent:
    """High-level agent wrapper with fluent API.

    Created via nbos.agent() or AgentBuilder.
    """

    def __init__(self, inner: PyAgent, tools: ToolRegistry) -> None:
        self._inner = inner
        self._tools = tools

    async def ask(self, prompt: str) -> str:
        return await self._inner.run_simple(prompt)

    async def run_simple(self, message: str) -> str:
        return await self.ask(message)

    async def chat(self, message: str) -> str:
        return await self.ask(message)

    async def react(self, task: str) -> str:
        return await self._inner.react(task)

    async def stream(self, task: str):
        return await self._inner.stream(task)

    @property
    def session(self) -> SessionManager:
        return SessionManager(self._inner)

    @property
    def tools(self) -> list[str]:
        return self._inner.list_tools()

    @property
    def config(self) -> dict[str, Any]:
        return self._inner.config()


class BrainOS(AbstractAsyncContextManager):
    """Main entry point - manages Bus lifecycle and agent creation.

    Auto-discovers config from ~/.bos/conf/config.toml and environment variables.
    """

    def __init__(
        self,
        *,
        config: dict[str, Any] | None = None,
        api_key: str | None = None,
        base_url: str | None = None,
        model: str | None = None,
    ) -> None:
        loader = PyConfigLoader()
        loader.discover()
        file_config = loader.load_sync()
        global_model = file_config.get("global_model", {})

        if config:
            self._api_key = api_key or config.get("api_key") or global_model.get("api_key")
            self._base_url = base_url or config.get("base_url") or global_model.get("base_url", DEFAULT_BASE_URL)
            self._model = model or config.get("model") or global_model.get("model", DEFAULT_MODEL)
        else:
            self._api_key = api_key or global_model.get("api_key")
            self._base_url = base_url or global_model.get("base_url", DEFAULT_BASE_URL)
            self._model = model or global_model.get("model", DEFAULT_MODEL)

        self._bus: PyBus | None = None
        self._registry = ToolRegistry()

    async def __aenter__(self) -> "BrainOS":
        self._bus = await PyBus.create(PyBusConfig())
        return self

    async def __aexit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        pass

    def agent(
        self,
        name: str = "assistant",
        *,
        model: str | None = None,
        system_prompt: str = "You are a helpful assistant.",
        temperature: float = 0.7,
        max_tokens: int | None = None,
        timeout_secs: int = 120,
        tools: list[ToolDef] | None = None,
        **kwargs,
    ) -> AgentBuilder:
        opts = {
            "name": name,
            "model": model or self._model,
            "base_url": self._base_url,
            "api_key": self._api_key,
            "system_prompt": system_prompt,
            "temperature": temperature,
            "max_tokens": max_tokens,
            "timeout_secs": timeout_secs,
        }
        opts.update(kwargs)
        return AgentBuilder(
            self._bus,
            opts,
        ).with_tools(*self._registry.list_tools(), *(tools or []))

    def register_global(self, *tools: ToolDef) -> "BrainOS":
        for t in tools:
            self._registry.add(t)
        return self

    def tools(self, *tools: ToolDef) -> "BrainOS":
        return self.register_global(*tools)

    @property
    def bus(self) -> PyBus:
        if self._bus is None:
            raise RuntimeError("BrainOS not started. Use 'async with' context.")
        return self._bus

    @property
    def registry(self) -> ToolRegistry:
        return self._registry

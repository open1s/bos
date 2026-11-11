"""Fluent builder API for nbos Agent Framework.

Based on jsbos patterns with fluent builder API.
"""

from __future__ import annotations

import json
import logging
import os
from typing import Any, Callable
from contextlib import AbstractAsyncContextManager

logging.basicConfig(level=logging.DEBUG, format='%(levelname)s %(name)s: %(message)s')
_LOG = logging.getLogger("nbos.core")

from nbos_native import Agent as PyAgent
from nbos_native import AgentConfig as PyAgentConfig
from nbos_native import AgentPlugin as PyAgentPlugin
from nbos_native import Bus as PyBus
from nbos_native import BusConfig as PyBusConfig
from nbos_native import PythonTool
from nbos_native import HookEvent, HookDecision, HookContext

from nbos.tool import ToolDef
from nbos.content import Content as NbosContent
from nbos.config import DEFAULT_BASE_URL, DEFAULT_MODEL, Config
from nbos.memory import Memory

# Import ContentPart for convenience
from nbos.content import ContentPart, Binary


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

    def unregister(self, name: str) -> "ToolRegistry":
        """Alias of remove, mirroring the JS registry surface."""
        return self.remove(name)

    def get(self, name: str) -> ToolDef | None:
        return self._tools.get(name)

    def has(self, name: str) -> bool:
        return name in self._tools

    def list(self) -> list[str]:
        return list(self._tools.keys())

    def list_tools(self) -> list[ToolDef]:
        return list(self._tools.values())

    def list_tool_defs(self) -> list[ToolDef]:
        """Alias of list_tools; every entry here is already a ToolDef."""
        return self.list_tools()

    def list_by_category(self, category: str) -> list[ToolDef]:
        """Tools whose optional category attribute matches."""
        return [t for t in self._tools.values() if getattr(t, "category", None) == category]

    def filter(self, predicate: Callable[[ToolDef], bool]) -> "ToolRegistry":
        """A new registry holding only the tools the predicate accepts."""
        return ToolRegistry([t for t in self._tools.values() if predicate(t)])

    def size(self) -> int:
        return len(self._tools)

    def clear(self) -> "ToolRegistry":
        self._tools.clear()
        return self

    def merge(self, other: "ToolRegistry") -> "ToolRegistry":
        for t in other.list_tools():
            self.add(t)
        return self

    def to_json(self) -> list[dict[str, Any]]:
        """Tool metadata as plain dictionaries, mirroring the JS registry."""
        return [
            {
                "name": t.name,
                "description": t.description,
                "parameters": t.parameters,
                "schema": t.schema,
            }
            for t in self._tools.values()
        ]


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

    def restore_full(self, path: str) -> "SessionManager":
        self._agent.restore_session(path)
        return self

    def compact(self, keep_recent: int = 10, max_summary_chars: int = 2000) -> "SessionManager":
        self._agent.compact_message_log(keep_recent, max_summary_chars)
        return self

    def clear(self) -> "SessionManager":
        self._agent.clear_session()
        return self

    def get_messages(self) -> list[dict]:
        return self._agent.get_messages()

    def add_message(self, role: str, content: str) -> "SessionManager":
        self._agent.add_message({"role": role, "content": content})
        return self

    def export(self) -> dict:
        return self._agent.session_state()

    def export_json(self) -> str:
        return json.dumps(self.export())

    def import_session(self, data: dict | str) -> "SessionManager":
        payload = data if isinstance(data, str) else json.dumps(data)
        self._agent.restore_session_json(payload)
        return self

    @property
    def context(self) -> dict:
        return self._agent.session_context()

    def set_context(self, context: dict) -> "SessionManager":
        self._agent.set_session_context(context)
        return self

    def clear_context(self) -> "SessionManager":
        self._agent.clear_session_context()
        return self


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
        self._prompt_sections: list[str] = []
        self._plan_tool = False
        self._deferred_tools: list[str] = []
        self._deferred_prefixes: list[str] = []

        opts = options or {}
        self._memory = opts.get("memory")
        self._recall_limit = opts.get("memory_recall_limit", 5)
        self._memory_metadata = opts.get("memory_metadata")
        self._auto_remember = opts.get("auto_remember", False)
        self._config = PyAgentConfig()
        self._config.name = opts.get("name", "assistant")
        self._config.model = opts.get("model", DEFAULT_MODEL)
        self._config.base_url = opts.get("base_url", DEFAULT_BASE_URL)
        self._config.api_key = opts.get("api_key") or ""
        self._config.system_prompt = opts.get("system_prompt", "You are a helpful assistant.")
        self._config.temperature = opts.get("temperature", 0.7)
        self._config.timeout_secs = opts.get("timeout_secs", 120)
        if "max_tokens" in opts and opts["max_tokens"] is not None:
            self._config.max_tokens = opts["max_tokens"]
        if "api_mode" in opts and opts["api_mode"] is not None:
            self._config.api_mode = opts["api_mode"]
        if "reasoning_effort" in opts and opts["reasoning_effort"] is not None:
            self._config.reasoning_effort = opts["reasoning_effort"]
        if opts.get("max_steps") is not None:
            self._config.max_steps = opts["max_steps"]
        if opts.get("persona_complete") is not None:
            self._config.persona_complete = bool(opts["persona_complete"])
        if opts.get("compaction_input_tokens") is not None:
            self._config.compaction_input_tokens = opts["compaction_input_tokens"]
        if opts.get("tool_output_cap") is not None:
            self._config.tool_output_cap = opts["tool_output_cap"]
        if opts.get("parallel_tool_calls") is not None:
            self._config.parallel_tool_calls = bool(opts["parallel_tool_calls"])
        if opts.get("web_search") is not None:
            self._config.web_search = bool(opts["web_search"])
        if opts.get("context_budget") is not None:
            self._config.context_budget = opts["context_budget"]
        if opts.get("skill_catalogue") is not None:
            self._config.skill_catalogue = bool(opts["skill_catalogue"])
        for provider in opts.get("fallbacks") or []:
            self._config.add_fallback(
                provider["base_url"],
                provider["model"],
                provider.get("api_key", ""),
            )

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

    def with_system(self, prompt: str) -> "AgentBuilder":
        """Alias for with_prompt (matches JS system)."""
        return self.with_prompt(prompt)

    def with_temperature(self, temperature: float) -> "AgentBuilder":
        self._config.temperature = temperature
        return self

    def with_max_tokens(self, max_tokens: int) -> "AgentBuilder":
        self._config.max_tokens = max_tokens
        return self

    def with_timeout(self, secs: int) -> "AgentBuilder":
        self._config.timeout_secs = secs
        return self

    def with_api_mode(self, mode: str) -> "AgentBuilder":
        self._config.api_mode = mode
        return self

    def with_reasoning_effort(self, effort: str) -> "AgentBuilder":
        self._config.reasoning_effort = effort
        return self

    def with_max_steps(self, steps: int) -> "AgentBuilder":
        """Set the maximum ReAct steps before a run is stopped."""
        self._config.max_steps = steps
        return self

    def with_persona_complete(self, complete: bool = True) -> "AgentBuilder":
        """Use the system prompt as the WHOLE prompt, with no appended guidance."""
        self._config.persona_complete = complete
        return self

    def with_compaction_input_tokens(self, tokens: int) -> "AgentBuilder":
        """Cap one compaction (summarizer) request's input."""
        self._config.compaction_input_tokens = tokens
        return self

    def with_tool_output_cap(self, bytes_cap: int) -> "AgentBuilder":
        """Cap the text one tool result may add to the model context."""
        self._config.tool_output_cap = bytes_cap
        return self

    def with_parallel_tool_calls(self, enabled: bool = True) -> "AgentBuilder":
        """Run one response's tool calls concurrently instead of one at a time."""
        self._config.parallel_tool_calls = enabled
        return self

    def with_web_search(self, enabled: bool = True) -> "AgentBuilder":
        """Offer the provider's hosted web-search tool (Responses protocol only)."""
        self._config.web_search = enabled
        return self

    def with_context_budget(self, tokens: int) -> "AgentBuilder":
        """Set the send-side history ceiling for mid-turn compaction."""
        self._config.context_budget = tokens
        return self

    def with_skill_catalogue(self, enabled: bool = True) -> "AgentBuilder":
        """Advertise skills as a catalogue instead of searching them."""
        self._config.skill_catalogue = enabled
        return self

    def with_fallback(
        self, base_url: str, model: str, api_key: str = ""
    ) -> "AgentBuilder":
        """Append an alternate endpoint tried when the primary provider fails."""
        self._config.add_fallback(base_url, model, api_key)
        return self

    def with_plan_tool(self, enabled: bool = True) -> "AgentBuilder":
        """Register the update_plan tool when the agent starts."""
        self._plan_tool = enabled
        return self

    def defer_tool(self, name: str) -> "AgentBuilder":
        """Keep a tool out of the advertised array; it stays callable."""
        self._deferred_tools.append(name)
        return self

    def defer_tool_prefix(self, prefix: str) -> "AgentBuilder":
        """Keep every tool whose name starts with the prefix out of the array."""
        self._deferred_prefixes.append(prefix)
        return self

    def with_memory(
        self,
        memory: Memory,
        limit: int = 5,
        metadata: dict[str, Any] | None = None,
        auto_remember: bool = False,
    ) -> "AgentBuilder":
        """Recall up to ``limit`` matches from ``memory`` on each text run."""
        self._memory = memory
        self._recall_limit = limit
        self._memory_metadata = metadata
        self._auto_remember = auto_remember
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

    def with_circuit_breaker(
        self, max_failures: int, cooldown_secs: int = 30
    ) -> "AgentBuilder":
        """Set circuit-breaker thresholds (matches JS circuitBreaker)."""
        self._config.circuit_breaker_max_failures = max_failures
        self._config.circuit_breaker_cooldown_secs = cooldown_secs
        return self

    def with_rate_limit(
        self, capacity: int, window_secs: int = 60, max_retries: int = 3
    ) -> "AgentBuilder":
        """Set the rate limiter (matches JS rateLimit)."""
        self._config.rate_limit_capacity = capacity
        self._config.rate_limit_window_secs = window_secs
        self._config.rate_limit_max_retries = max_retries
        return self

    def with_config(self, config: dict[str, Any]) -> "AgentBuilder":
        """Apply a config mapping in one call (matches JS withConfig)."""
        if config.get("name"):
            self._config.name = config["name"]
        if config.get("model"):
            self._config.model = config["model"]
        if config.get("base_url"):
            self._config.base_url = config["base_url"]
        if config.get("api_key"):
            self._config.api_key = config["api_key"]
        if config.get("system_prompt"):
            self._config.system_prompt = config["system_prompt"]
        if config.get("temperature") is not None:
            self._config.temperature = config["temperature"]
        if config.get("timeout_secs"):
            self._config.timeout_secs = config["timeout_secs"]
        if config.get("max_tokens"):
            self._config.max_tokens = config["max_tokens"]
        if config.get("api_mode"):
            self._config.api_mode = config["api_mode"]
        if config.get("reasoning_effort"):
            self._config.reasoning_effort = config["reasoning_effort"]
        if config.get("max_steps") is not None:
            self._config.max_steps = config["max_steps"]
        if config.get("persona_complete") is not None:
            self._config.persona_complete = bool(config["persona_complete"])
        if config.get("compaction_input_tokens") is not None:
            self._config.compaction_input_tokens = config["compaction_input_tokens"]
        if config.get("tool_output_cap") is not None:
            self._config.tool_output_cap = config["tool_output_cap"]
        if config.get("parallel_tool_calls") is not None:
            self._config.parallel_tool_calls = bool(config["parallel_tool_calls"])
        if config.get("web_search") is not None:
            self._config.web_search = bool(config["web_search"])
        if config.get("context_budget") is not None:
            self._config.context_budget = config["context_budget"]
        if config.get("skill_catalogue") is not None:
            self._config.skill_catalogue = bool(config["skill_catalogue"])
        for provider in config.get("fallbacks") or []:
            self._config.add_fallback(
                provider["base_url"],
                provider["model"],
                provider.get("api_key", ""),
            )
        if config.get("circuit_breaker"):
            cb = config["circuit_breaker"]
            self.with_circuit_breaker(cb["max_failures"], cb.get("cooldown_secs", 30))
        if config.get("rate_limit"):
            rl = config["rate_limit"]
            self.with_rate_limit(
                rl["capacity"], rl.get("window_secs", 60), rl.get("max_retries", 3)
            )
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

    def skill(
        self,
        name: str,
        content: str,
        description: str | None = None,
        path: str | None = None,
    ) -> "AgentBuilder":
        """Register an inline skill (no SKILL.md file needed)."""
        self._skills.append(
            {
                "name": name,
                "content": content,
                "description": description or name,
                "path": path,
            }
        )
        return self

    def with_mcp(self, namespace: str, command: str, args: list[str]) -> "AgentBuilder":
        self._mcp_servers.append({"namespace": namespace, "command": command, "args": args, "type": "process"})
        return self

    def with_mcp_http(self, namespace: str, url: str) -> "AgentBuilder":
        self._mcp_servers.append({"namespace": namespace, "url": url, "type": "http"})
        return self

    def with_bash(
        self,
        name: str = "bash",
        workspace_root: str | None = None,
        jobs: object | None = None,
        session: str | None = None,
    ) -> "AgentBuilder":
        """Add a sandboxed bash tool.

        Pass jobs and session to enable run_in_background and route the
        resulting jobs into that JobRegistry.
        """
        self._config._bash_tool = {
            "name": name,
            "workspace_root": workspace_root,
            "jobs": jobs,
            "session": session,
        }
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
                metadata=json.dumps(td.metadata) if td.metadata else None,
            )
            await self._inner.add_tool(py_tool)

        if hasattr(self._config, "_bash_tool"):
            bash_cfg = self._config._bash_tool
            await self._inner.add_bash_tool(
                bash_cfg["name"],
                bash_cfg.get("workspace_root"),
                bash_cfg.get("jobs"),
                bash_cfg.get("session"),
            )

        for event_name, callback in self._hooks:
            self._inner.register_hook(HookEvent(event_name), callback)

        for p in self._plugins:
            plugin_obj = PyAgentPlugin(
                name=p.get("name", "plugin"),
                on_llm_request=p.get("on_llm_request"),
                on_llm_response=p.get("on_llm_response"),
                on_tool_call=p.get("on_tool_call"),
                on_tool_result=p.get("on_tool_result"),
                on_stream_token=p.get("on_stream_token"),
                on_turn_start=p.get("on_turn_start"),
                on_turn_end=p.get("on_turn_end"),
            )
            self._inner.register_plugin(plugin_obj)

        for s in self._skills:
            if "dir_path" in s:
                await self._inner.register_skills_from_dir(s["dir_path"])
            else:
                self._inner.register_skill(
                    s["name"],
                    s.get("description") or s["name"],
                    s["content"],
                    s.get("path"),
                )

        for name in self._deferred_tools:
            self._inner.defer_tool(name)
        for prefix in self._deferred_prefixes:
            self._inner.defer_tool_prefix(prefix)
        if self._plan_tool:
            self._inner.add_plan_tool()
        for section in self._prompt_sections:
            self._inner.add_live_prompt_section(section)

        for m in self._mcp_servers:
            if m["type"] == "process":
                await self._inner.add_mcp_server(m["namespace"], m["command"], m["args"])
            else:
                await self._inner.add_mcp_server_http(m["namespace"], m["url"])

        return Agent(
            self._inner,
            self._tools,
            self._memory,
            self._recall_limit,
            self._memory_metadata,
            self._auto_remember,
        )

    def _resolve_content(self, input_val: str | NbosContent) -> str:
        """Convert input to string for Rust backend.

        If input is a NbosContent, convert to JSON.
        Otherwise return as-is.
        """
        if isinstance(input_val, NbosContent):
            return input_val.to_json()
        return input_val

    async def ask(self, prompt: str | NbosContent) -> str:
        if not self._inner:
            await self.start()
        return await Agent(
            self._inner,
            self._tools,
            self._memory,
            self._recall_limit,
            self._memory_metadata,
            self._auto_remember,
        ).ask(prompt)

    async def chat(self, message: str | NbosContent) -> str:
        return await self.ask(message)

    async def react(self, task: str | NbosContent) -> str:
        if not self._inner:
            await self.start()
        return await Agent(
            self._inner,
            self._tools,
            self._memory,
            self._recall_limit,
            self._memory_metadata,
            self._auto_remember,
        ).react(task)

    async def stream(self, task: str | NbosContent):
        if not self._inner:
            await self.start()
        return await Agent(
            self._inner,
            self._tools,
            self._memory,
            self._recall_limit,
            self._memory_metadata,
            self._auto_remember,
        ).stream(task)


class Agent:
    """High-level agent wrapper with fluent API.

    Created via nbos.agent() or AgentBuilder.
    """

    def __init__(
        self,
        inner: PyAgent,
        tools: ToolRegistry,
        memory: Memory | None = None,
        recall_limit: int = 5,
        memory_metadata: dict[str, Any] | None = None,
        auto_remember: bool = False,
    ) -> None:
        self._inner = inner
        self._tools = tools
        self._memory = memory
        self._recall_limit = recall_limit
        self._memory_metadata = memory_metadata
        self._auto_remember = auto_remember

    def _resolve_content(self, input_val: str | NbosContent) -> str:
        """Convert input to string for Rust backend.

        If input is a NbosContent, convert to JSON.
        Otherwise return as-is.
        """
        if isinstance(input_val, NbosContent):
            return input_val.to_json()
        return input_val

    def _with_memory(self, prompt: str | NbosContent, content: str) -> str:
        """Prepend recalled memory to text prompts; leave multimodal untouched."""
        if self._memory is None or isinstance(prompt, NbosContent):
            return content
        block = self._memory.recall_block(
            prompt, self._recall_limit, self._memory_metadata
        )
        return f"{block}\n\n{content}" if block else content

    def _remember_exchange(self, prompt: str | NbosContent, reply: str) -> None:
        if (
            not self._auto_remember
            or self._memory is None
            or not isinstance(prompt, str)
            or not prompt.strip()
            or not reply.strip()
        ):
            return
        note = f"User: {prompt.strip()}\nAssistant: {reply.strip()[:4000]}"
        self._memory.add(note)

    def with_memory(
        self,
        memory: Memory,
        limit: int = 5,
        metadata: dict[str, Any] | None = None,
        auto_remember: bool = False,
    ) -> "Agent":
        """Attach a store that recalls up to ``limit`` items per text run."""
        self._memory = memory
        self._recall_limit = limit
        self._memory_metadata = metadata
        self._auto_remember = auto_remember
        return self

    def remember(self, content: str, metadata: Any = None) -> dict:
        """Store one memory and return the stored item."""
        if self._memory is None:
            raise RuntimeError("no memory store is attached")
        return self._memory.add(content, metadata)

    def forget(self, item_id: str) -> bool:
        """Remove one memory item by id."""
        return self._memory.remove(item_id) if self._memory is not None else False

    def forget_all(self) -> bool:
        """Clear the attached memory store."""
        if self._memory is None:
            return False
        had_items = not self._memory.is_empty()
        self._memory.clear()
        return had_items

    def recall(self, query: str, limit: int = 5) -> list[dict]:
        """Recall items relevant to ``query``, honouring the metadata filter."""
        if self._memory is None:
            return []
        return self._memory.search(query, limit, self._memory_metadata)

    def recalled_context(self, query: str, limit: int = 5) -> str | None:
        """Recall matching items formatted for a prompt, or None."""
        if self._memory is None:
            return None
        block = self._memory.recall_block(query, limit, self._memory_metadata)
        return block or None

    async def ask(self, prompt: str | NbosContent) -> str:
        content = self._resolve_content(prompt)
        reply = await self._inner.run_simple(self._with_memory(prompt, content))
        self._remember_exchange(prompt, reply)
        return reply

    async def run_simple(self, message: str | NbosContent) -> str:
        return await self.ask(message)

    async def chat(self, message: str | NbosContent) -> str:
        return await self.ask(message)

    async def react(self, task: str | NbosContent) -> str:
        content = self._resolve_content(task)
        reply = await self._inner.react(self._with_memory(task, content))
        self._remember_exchange(task, reply)
        return reply

    async def stream(self, task: str | NbosContent):
        content = self._resolve_content(task)
        return await self._inner.stream(self._with_memory(task, content))

    async def stream_collect(self, task: str | NbosContent) -> list:
        """Collect all stream tokens into a list (matches JS streamCollect)."""
        tokens = []
        stream = await self.stream(task)
        async for token in stream:
            tokens.append(token)
        return tokens

    def stop(self, clear_session: bool = False) -> bool:
        """Cooperatively stop the agent; suppresses the next run."""
        return self._inner.stop(clear_session)

    def close(self) -> None:
        """Release runtime extensions and stop the agent."""
        self._inner.close()

    async def compact_now(self, retain_tokens: int = 0) -> dict | None:
        """Ask the model to compact the session into a checkpoint now."""
        return await self._inner.compact_now(retain_tokens)

    @property
    def is_running(self) -> bool:
        """Whether an agent call is currently in flight."""
        return bool(self._inner.is_running())

    @property
    def session(self) -> SessionManager:
        return SessionManager(self._inner)

    @property
    def tools(self) -> list[str]:
        return self._inner.list_tools()

    @property
    def config(self) -> dict[str, Any]:
        return self._inner.config()

    @property
    def tool_names(self) -> list[str]:
        """Alias for :attr:`tools`, matching JavaScript's `toolNames`."""
        return self._inner.list_tools()

    @property
    def metrics(self) -> dict[str, Any]:
        """Performance metrics collected across LLM calls (timings in microseconds)."""
        return self._inner.get_perf_metrics()

    def reset_metrics(self) -> "Agent":
        """Reset the performance metrics collected so far."""
        self._inner.reset_perf_metrics()
        return self

    async def list_mcp_tools(self) -> list[dict]:
        """List tools exposed by connected MCP servers."""
        return await self._inner.list_mcp_tools()

    async def list_mcp_resources(self, namespace: str) -> list[dict]:
        """List resources exposed by the MCP server in the given namespace."""
        return await self._inner.list_mcp_resources(namespace)

    async def list_mcp_prompts(self) -> list[dict]:
        """List prompts exposed by connected MCP servers."""
        return await self._inner.list_mcp_prompts()

    def plan_items(self) -> list[dict]:
        """Snapshot the agent's working plan as {text, status} items."""
        return self._inner.plan_items()

    def set_plan(self, items: list[dict]) -> "Agent":
        """Replace the working plan; an empty list clears it."""
        self._inner.set_plan(items)
        return self

    def clear_plan(self) -> "Agent":
        """Forget the whole working plan."""
        self._inner.clear_plan()
        return self

    def add_plan_tool(self) -> bool:
        """Register the update_plan tool against this agent's plan store."""
        return self._inner.add_plan_tool()

    def steer(self, content: str, ticket: int) -> bool:
        """Queue a message for the running turn's next step."""
        return self._inner.steer(content, ticket)

    def steer_depth(self) -> int:
        """Number of steered messages waiting for the running turn."""
        return self._inner.steer_depth()

    def take_undelivered_steers(self) -> list[int]:
        """Take back every steered message no step ever delivered."""
        return self._inner.take_undelivered_steers()

    def defer_tool(self, name: str) -> "Agent":
        """Keep a tool out of the advertised array; it stays callable."""
        self._inner.defer_tool(name)
        return self

    def defer_tool_prefix(self, prefix: str) -> "Agent":
        """Keep every tool whose name starts with the prefix out of the array."""
        self._inner.defer_tool_prefix(prefix)
        return self

    def is_deferred_tool(self, name: str) -> bool:
        """Whether a tool is withheld from the advertised array."""
        return self._inner.is_deferred_tool(name)

    def has_deferred_tools(self) -> bool:
        """Whether any registered tool is withheld from the advertised array."""
        return self._inner.has_deferred_tools()

    def remove_tools(self, names: list[str]) -> int:
        """Remove asynchronous tools by name; returns how many were removed."""
        return self._inner.remove_tools(names)

    async def add_live_tool(self, tool: ToolDef) -> "Agent":
        """Register a tool that a running turn can see immediately."""
        py_tool = PythonTool(
            name=tool.name,
            description=tool.description,
            parameters=json.dumps(tool.parameters),
            schema=json.dumps(tool.schema),
            callback=tool.callback,
            metadata=json.dumps(tool.metadata) if tool.metadata else None,
        )
        await self._inner.add_live_tool(py_tool)
        self._tools.add(tool)
        return self

    def remove_live_tools(self, names: list[str]) -> int:
        """Remove live extension tools by name."""
        return self._inner.remove_live_tools(names)

    def tool_definitions(self) -> list[dict]:
        """Every registered tool as a full definition, including live/MCP tools."""
        return self._inner.tool_definitions()

    def model_tool_definitions(self) -> list[dict]:
        """The tool array the model is actually offered, as full definitions."""
        return self._inner.model_tool_definitions()

    def add_live_prompt_section(self, section: str) -> "Agent":
        """Append a live extension prompt fragment."""
        self._inner.add_live_prompt_section(section)
        return self

    def remove_live_prompt_section(self, section: str) -> bool:
        """Remove one live extension prompt fragment."""
        return self._inner.remove_live_prompt_section(section)

    def register_skill(
        self,
        name: str,
        description: str,
        content: str,
        path: str | None = None,
    ) -> "Agent":
        """Register an inline skill (no SKILL.md file needed)."""
        self._inner.register_skill(name, description, content, path)
        return self

    def add_live_skill(
        self,
        name: str,
        description: str,
        content: str,
        path: str | None = None,
    ) -> "Agent":
        """Register an inline skill that a running turn can see immediately."""
        self._inner.add_live_skill(name, description, content, path)
        return self

    async def add_live_skills_from_dir(self, dir_path: str) -> int:
        """Load every skill in a directory into the live extension overlay."""
        return await self._inner.add_live_skills_from_dir(dir_path)

    def remove_live_skills_by_paths(self, paths: list[str]) -> int:
        """Remove live skills whose directory is one of the given paths."""
        return self._inner.remove_live_skills_by_paths(paths)

    def live_skills(self) -> list[dict]:
        """Every skill in the live extension overlay."""
        return self._inner.live_skills()

    def remove_skills_by_names(self, names: list[str]) -> int:
        """Remove skills by name from the build's list and the live overlay."""
        return self._inner.remove_skills_by_names(names)

    def list_skills(self) -> list[dict]:
        """Snapshot every registered skill."""
        return self._inner.list_skills()

    def context_budget(self, tokens: int | None = None):
        """Read, or with tokens set, the send-side context budget."""
        if tokens is None:
            return self._inner.context_budget()
        self._inner.set_context_budget(tokens)
        return self

    def set_context_budget(self, tokens: int) -> "Agent":
        """Set the send-side history ceiling for mid-turn compaction."""
        self._inner.set_context_budget(tokens)
        return self

    def skill_catalogue(self, enabled: bool | None = None):
        """Read, or with enabled choose, catalogue vs on-demand search."""
        if enabled is None:
            return self._inner.skill_catalogue()
        self._inner.set_skill_catalogue(enabled)
        return self

    def set_skill_catalogue(self, enabled: bool) -> "Agent":
        """Choose between the skill catalogue and on-demand find_skill."""
        self._inner.set_skill_catalogue(enabled)
        return self

    def add_goal_tools(self, store: object) -> int:
        """Register the get_goal and update_goal tools against a store."""
        return self._inner.add_goal_tools(store)

    def add_job_tools(self, jobs: object, session: str = "default") -> int:
        """Register the job_output and job_kill tools against a roster."""
        return self._inner.add_job_tools(jobs, session)

    async def add_bash_tool(
        self,
        name: str = "bash",
        workspace_root: str | None = None,
        jobs: object | None = None,
        session: str | None = None,
    ) -> "Agent":
        """Register a sandboxed bash tool, optionally routing background jobs."""
        await self._inner.add_bash_tool(name, workspace_root, jobs, session)
        return self


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
        **options: Any,
    ) -> None:
        self._options = options
        self._config = Config()
        self._config.discover().load()
        global_model = self._config.global_model
        overrides = config or {}

        # Explicit option beats the standard environment variable, which beats
        # the discovered config file. The empty-string default keeps agent
        # construction working on machines with no BOS config at all.
        self._api_key = (
            api_key
            or overrides.get("api_key")
            or os.environ.get("OPENAI_API_KEY")
            or global_model.get("api_key")
            or ""
        )
        self._base_url = (
            base_url
            or overrides.get("base_url")
            or global_model.get("base_url", DEFAULT_BASE_URL)
        )
        self._model = (
            model or overrides.get("model") or global_model.get("model", DEFAULT_MODEL)
        )

        self._bus: PyBus | None = None
        self._started = False
        self._registry = ToolRegistry()

    @classmethod
    async def create(cls, **options: Any) -> "BrainOS":
        """Construct and start an instance in one call."""
        instance = cls(**options)
        await instance.start()
        return instance

    async def start(self) -> "BrainOS":
        """Start the bus if needed; safe to call more than once."""
        if self._bus is None:
            self._bus = await PyBus.create(
                PyBusConfig(
                    mode=self._options.get("mode") or self._options.get("bus_mode") or "peer",
                    connect=self._options.get("connect") or self._options.get("bus_connect"),
                    listen=self._options.get("listen") or self._options.get("bus_listen"),
                    peer=self._options.get("peer") or self._options.get("bus_peer"),
                )
            )
        self._started = True
        return self

    async def stop(self) -> None:
        """Close the bus and return to the stopped state."""
        if self._bus is not None:
            await self._bus.close()
            self._bus = None
        self._started = False

    async def create_bus(self, **options: Any) -> Any:
        """Build a standalone managed bus, mirroring the JS createBus helper."""
        from nbos.bus import BusManager

        return await BusManager(**options).start()

    async def __aenter__(self) -> "BrainOS":
        return await self.start()

    async def __aexit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        await self.stop()

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
    def is_started(self) -> bool:
        return self._started

    @property
    def config(self) -> Config:
        return self._config

    @property
    def bus(self) -> PyBus:
        if self._bus is None:
            raise RuntimeError("BrainOS not started. Use 'async with' context.")
        return self._bus

    @property
    def registry(self) -> ToolRegistry:
        return self._registry

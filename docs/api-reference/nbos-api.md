# BrainOS Python API Reference

This document provides the complete API reference for the BrainOS Python bindings (`nbos` package).

## Main Entry Point

### BrainOS

Main entry point — manages Bus lifecycle, config auto-discovery, and global tool registry.

#### Constructor

```python
BrainOS(*, config=None, api_key=None, base_url=None, model=None)
```

Parameters:
- `config` (dict, optional): Inline configuration overrides
- `api_key` (str, optional): API key for LLM provider
- `base_url` (str, optional): Base URL for LLM API
- `model` (str, optional): Model name to use

Config is auto-discovered from `~/.bos/conf/config.toml` and environment variables.

#### Context Manager

```python
async with BrainOS() as brain:
    # brain.bus is available
    agent = brain.agent("assistant")
```

#### Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `agent(name, **options)` | Create an AgentBuilder | `AgentBuilder` |
| `register_global(*tools)` | Register tools available to all agents | `BrainOS` |
| `tools(*tools)` | Alias for `register_global` | `BrainOS` |

#### Properties

| Property | Type | Description |
|----------|------|-------------|
| `bus` | `Bus` | The underlying Bus instance |
| `registry` | `ToolRegistry` | Global tool registry |

#### Example

```python
from nbos import BrainOS, tool

@tool("Add two numbers")
def add(a: int, b: int) -> int:
    return a + b

async def main():
    async with BrainOS() as brain:
        agent = (
            brain.agent("assistant")
            .register(add)
            .with_prompt("You are a helpful math assistant.")
        )
        result = await agent.ask("What is 2+2?")
        print(result)

import asyncio
asyncio.run(main())
```

---

## AgentBuilder

Fluent builder for creating agents with chainable configuration.

#### Constructor

```python
AgentBuilder(bus, options=None)
```

#### Fluent Configuration Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `name(name)` | Set agent name | `AgentBuilder` |
| `with_model(model)` | Set model name | `AgentBuilder` |
| `with_base_url(url)` | Set base URL | `AgentBuilder` |
| `with_api_key(key)` | Set API key | `AgentBuilder` |
| `with_prompt(prompt)` | Set system prompt | `AgentBuilder` |
| `with_temperature(temp)` | Set temperature | `AgentBuilder` |
| `with_max_tokens(tokens)` | Set max tokens | `AgentBuilder` |
| `with_timeout(secs)` | Set timeout | `AgentBuilder` |
| `with_tools(*tools)` | Register tools | `AgentBuilder` |
| `register(*tools)` | Alias for `with_tools` | `AgentBuilder` |
| `with_resilience(...)` | Configure circuit breaker + rate limiter | `AgentBuilder` |
| `hook(event, callback)` | Register a lifecycle hook | `AgentBuilder` |
| `with_hooks(hooks)` | Register multiple hooks | `AgentBuilder` |
| `plugin(name, **handlers)` | Register a plugin | `AgentBuilder` |
| `with_plugins(*plugins)` | Register multiple plugins | `AgentBuilder` |
| `with_skills_dir(path)` | Load skills from directory | `AgentBuilder` |
| `skill(name, content)` | Add inline skill | `AgentBuilder` |
| `with_mcp(ns, cmd, args)` | Add MCP server (process) | `AgentBuilder` |
| `with_mcp_http(ns, url)` | Add MCP server (HTTP) | `AgentBuilder` |
| `with_bash(name, workspace_root)` | Add bash tool | `AgentBuilder` |

#### Execution Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `start()` | Build and initialize the agent | `Agent` |
| `ask(prompt)` | Auto-start + run simple | `str` |
| `chat(message)` | Alias for `ask` | `str` |
| `react(task)` | Auto-start + run ReAct | `str` |
| `stream(task)` | Auto-start + stream tokens | `AsyncIterator` |

#### Example

```python
agent = (
    AgentBuilder(brain.bus)
    .name("assistant")
    .with_tools(add, multiply)
    .with_prompt("You are a math expert.")
    .with_temperature(0.5)
    .with_hooks({"BeforeToolCall": my_hook})
    .with_bash("bash")
    .start()
)
result = await agent.ask("What is 15 + 23?")
```

---

## Agent

High-level agent wrapper with fluent API. Created via `BrainOS.agent()` or `AgentBuilder.start()`.

#### Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `ask(prompt)` | Run simple task | `str` |
| `run_simple(message)` | Alias for `ask` | `str` |
| `chat(message)` | Alias for `ask` | `str` |
| `react(task)` | Run with ReAct reasoning | `str` |
| `stream(task)` | Stream response tokens | `AsyncIterator` |

#### Properties

| Property | Type | Description |
|----------|------|-------------|
| `session` | `SessionManager` | Session management |
| `tools` | `list[str]` | Registered tool names |
| `config` | `dict` | Agent configuration |

#### Example

```python
agent = brain.agent("assistant").register(add_tool)
result = await agent.ask("What is 2+2?")

# Session management
session = agent.session
session.save_full("./session.json")
session.restore_full("./session.json")
session.compact(2, 500)
messages = session.get_messages()
```

---

## SessionManager

Session management for an agent.

#### Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `save(path)` | Save message log | `SessionManager` |
| `restore(path)` | Restore message log | `SessionManager` |
| `save_full(path)` | Save full session | `SessionManager` |
| `restore_full(path)` | Restore full session | `SessionManager` |
| `compact(keep_recent, max_summary_chars)` | Compact conversation | `SessionManager` |
| `clear()` | Clear session context | `SessionManager` |
| `get_messages()` | Get all messages | `list[dict]` |
| `add_message(role, content)` | Add a message | `SessionManager` |
| `export()` | Export session state | `dict` |

#### Properties

| Property | Type | Description |
|----------|------|-------------|
| `context` | `dict` | Session context |

---

## @tool() / ToolDef

#### `@tool(description, *, name=None, schema=None)`

Decorator to create a tool from a function. Supports both sync and async callbacks (auto-detected).

```python
from nbos import tool

# Sync tool
@tool("Add two numbers")
def add(a: int, b: int) -> int:
    return a + b

# Async tool (automatically detected and awaited)
@tool("Fetch weather data")
async def get_weather(city: str) -> dict:
    import asyncio
    await asyncio.sleep(0.1)  # Simulate API call
    return {"city": city, "temperature": 22}
```

#### ToolDef

Manual tool creation with async callback support.

```python
from nbos import ToolDef

# Sync callback
multiply = ToolDef(
    name="multiply",
    description="Multiply two numbers",
    callback=lambda args: args["a"] * args["b"],
    parameters={"a": {"type": "number"}, "b": {"type": "number"}},
    schema={"type": "object", "properties": {"a": {"type": "number"}, "b": {"type": "number"}}},
)

# Async callback (auto-detected via inspect.iscoroutinefunction)
async def async_weather_callback(args):
    import asyncio
    await asyncio.sleep(0.1)
    return {"city": args.get("city", "Unknown"), "temp": 68}

weather = ToolDef(
    name="weather_async",
    description="Get weather from async API",
    callback=async_weather_callback,
    parameters={"type": "object", "properties": {"city": {"type": "string"}}},
    schema={"type": "object", "properties": {"city": {"type": "string"}}},
)
```

#### ToolResult

```python
from nbos import ToolResult

# Success
result = ToolResult.success(data, metadata={"key": "value"})

# Error
result = ToolResult.error("Something went wrong")
```

---

## ToolRegistry

Registry for managing multiple tools.

#### Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `add(tool)` | Add a tool | `ToolRegistry` |
| `register(tool)` | Alias for `add` | `ToolRegistry` |
| `remove(name)` | Remove a tool | `ToolRegistry` |
| `get(name)` | Get tool by name | `ToolDef \| None` |
| `has(name)` | Check if tool exists | `bool` |
| `list()` | List tool names | `list[str]` |
| `list_tools()` | List tool definitions | `list[ToolDef]` |
| `size()` | Count tools | `int` |
| `clear()` | Clear all tools | `ToolRegistry` |
| `merge(other)` | Merge another registry | `ToolRegistry` |

---

## AgentConfig

Configuration for creating an agent (low-level).

```python
from nbos import AgentConfig

config = AgentConfig()
config.name = "assistant"
config.model = "gpt-4"
config.base_url = "https://api.openai.com/v1"
config.api_key = "sk-..."
config.system_prompt = "You are helpful."
config.temperature = 0.7
config.timeout_secs = 120
```

#### Resilience Fields

| Field | Default | Description |
|-------|---------|-------------|
| `rate_limit_capacity` | 40 | Max requests per window |
| `rate_limit_window_secs` | 60 | Window duration |
| `rate_limit_max_retries` | 3 | Retry attempts on 429 |
| `circuit_breaker_max_failures` | 5 | Failures before open circuit |
| `circuit_breaker_cooldown_secs` | 30 | Seconds before half-open |

---

## PyAgent (Native Agent)

The native Rust-backed agent class from `nbos_native`.

#### Methods

| Method | Description |
|--------|-------------|
| `create(config, bus)` | Create agent (async class method) |
| `run_simple(task)` | Run simple task |
| `react(task)` | Run ReAct reasoning |
| `stream(task)` | Stream tokens |
| `add_tool(tool)` | Register PythonTool |
| `add_bash_tool(name, workspace_root)` | Add bash tool |
| `add_mcp_server(ns, cmd, args)` | Add MCP server (process) |
| `add_mcp_server_http(ns, url)` | Add MCP server (HTTP) |
| `register_hook(event, callback)` | Register lifecycle hook |
| `register_plugin(plugin)` | Register plugin |
| `register_skills_from_dir(path)` | Load skills from dir |
| `list_tools()` | List tool names |
| `config()` | Get config as dict |
| `save_message_log(path)` | Save messages |
| `restore_message_log(path)` | Restore messages |
| `save_session(path)` | Save full session |
| `restore_session(path)` | Restore full session |
| `compact_message_log()` | Compact messages |
| `clear_session_context()` | Clear session |
| `get_messages()` | Get messages |
| `session_context()` | Get session context |
| `session_state()` | Export session state |
| `token_usage()` | Get token usage |
| `token_budget_report()` | Get budget report |

---

## Bus / BusConfig

#### Bus

```python
from nbos import Bus, BusConfig

bus = await Bus.create(BusConfig())
```

#### BusConfig

| Field | Default | Description |
|-------|---------|-------------|
| `mode` | `"peer"` | Bus mode: peer, client, server |
| `connect` | `None` | Connection addresses |
| `listen` | `None` | Listen addresses |
| `peer` | `None` | Peer ID |

#### Bus Methods

| Method | Description |
|--------|-------------|
| `create(config)` | Create bus (async class method) |
| `publish_text(topic, payload)` | Publish text |
| `publish_json(topic, data)` | Publish JSON |
| `create_publisher(topic)` | Create publisher |
| `create_subscriber(topic)` | Create subscriber |
| `create_query(topic)` | Create query client |
| `create_queryable(topic)` | Create queryable server |
| `create_caller(name)` | Create caller client |
| `create_callable(uri)` | Create callable server |

---

## Publisher / Subscriber

#### Publisher

| Method | Description |
|--------|-------------|
| `publish_text(payload)` | Publish text |
| `publish_json(data)` | Publish JSON |

#### Subscriber

| Method | Description |
|--------|-------------|
| `recv()` | Receive message (blocking) |
| `recv_with_timeout_ms(ms)` | Receive with timeout |
| `recv_json_with_timeout_ms(ms)` | Receive JSON with timeout |
| `run(callback)` | Run callback loop |
| `run_json(callback)` | Run JSON callback loop |

---

## Query / Queryable

#### Query

| Method | Description |
|--------|-------------|
| `query_text(payload)` | Send text query |
| `query_text_timeout_ms(payload, ms)` | Query with timeout |

#### Queryable

| Method | Description |
|--------|-------------|
| `start()` | Start server |
| `run(handler)` | Run with handler |
| `run_json(handler)` | Run with JSON handler |

---

## Caller / Callable

#### Caller

| Method | Description |
|--------|-------------|
| `call_text(payload)` | Call remote service |

#### Callable

| Method | Description |
|--------|-------------|
| `start()` | Start server |
| `run(handler)` | Run with handler |
| `run_json(handler)` | Run with JSON handler |
| `is_started()` | Check if running |

---

## ConfigLoader

```python
from nbos import ConfigLoader

loader = ConfigLoader()
loader.discover()
loader.add_file("app.toml")
loader.add_inline({"agent": {"model": "gpt-4"}})
config = loader.load_sync()
```

#### Methods

| Method | Description |
|--------|-------------|
| `discover()` | Auto-discover config files |
| `add_file(path)` | Add config file |
| `add_directory(path)` | Add config directory |
| `add_inline(data)` | Add inline config |
| `reset()` | Reset configuration |
| `load_sync()` | Load config (returns dict) |
| `reload_sync()` | Reload config |

---

## McpClient

```python
from nbos import McpClient

# Process-based
client = await McpClient.spawn("npx", ["-y", "server-filesystem", "/tmp"])
await client.initialize()

# HTTP
client = McpClient.connect_http("http://127.0.0.1:8000/mcp")
await client.initialize()
```

#### Methods

| Method | Description |
|--------|-------------|
| `spawn(command, args)` | Spawn MCP server (static) |
| `connect_http(url)` | Connect via HTTP (static) |
| `initialize()` | Initialize connection |
| `list_tools()` | List available tools |
| `call_tool(name, args_json)` | Call a tool |
| `list_prompts()` | List prompts |
| `list_resources()` | List resources |
| `read_resource(uri)` | Read resource by URI |

---

## Hooks

Hooks support both sync and async callbacks (auto-detected).

#### HookEvent

| Event | Description |
|-------|-------------|
| `BeforeToolCall` | Before tool execution |
| `AfterToolCall` | After tool execution |
| `BeforeLlmCall` | Before LLM API call |
| `AfterLlmCall` | After LLM API call |
| `OnMessage` | For each message |
| `OnComplete` | When agent completes |
| `OnError` | When error occurs |

#### HookDecision

| Decision | Description |
|----------|-------------|
| `HookDecision("Continue", None)` | Proceed normally |
| `HookDecision("Abort", None)` | Abort operation |
| `HookDecision("Error", "message")` | Return error |

#### HookContext

| Property | Type | Description |
|----------|------|-------------|
| `agent_id` | `str` | Agent identifier |
| `data` | `dict[str, str]` | Event data |

#### Example - Sync Hook

```python
from nbos import HookEvent, HookDecision

def my_hook(event, ctx):
    print(f"[{event.value}] agent={ctx.agent_id}")
    return HookDecision("Continue", None)

agent = brain.agent("assistant").hook("BeforeToolCall", my_hook)
```

#### Example - Async Hook

Async hooks are automatically detected and awaited.

```python
import asyncio

async def async_rate_limit_hook(event, ctx):
    await asyncio.sleep(0.01)  # Simulate async check
    print(f"[Async Hook:{event}] Rate limit check passed")
    return "Continue"

async def async_logging_hook(event, ctx):
    await asyncio.sleep(0.01)  # Simulate async logging
    print(f"[Async Hook:{event}] Logged response")
    return "Continue"

agent = brain.agent("assistant").with_hooks({
    "BeforeLlmCall": async_rate_limit_hook,
    "AfterLlmCall": async_logging_hook,
})
```

---

## Plugins

Plugins support both sync and async callbacks (auto-detected). They intercept LLM requests/responses and tool calls/results.

#### AgentPlugin

Plugins are registered as dictionaries with handler functions:

```python
from nbos import AgentPlugin

# Sync plugin
def sync_on_llm_request(request):
    print(f"[Plugin] LLM Request: model={request.model}")
    return request

async def async_on_llm_request(request):
    await asyncio.sleep(0.01)  # Simulate async enrichment
    print(f"[Async Plugin] LLM Request: model={request.model}")
    request.temperature = 0.7
    return request

async def async_on_llm_response(response):
    await asyncio.sleep(0.01)  # Simulate async analysis
    print(f"[Async Plugin] LLM Response: type={response.response_type}")
    return response

# Register as dictionary (supports async handlers)
plugin = {
    "name": "AsyncEnricher",
    "on_llm_request": async_on_llm_request,
    "on_llm_response": async_on_llm_response,
}
agent = brain.agent("assistant").with_plugins(plugin)
```

#### Wrapper Classes

| Class | Description |
|-------|-------------|
| `LlmRequestWrapper` | LLM request interceptor - fields: `model`, `temperature`, `max_tokens`, `top_p`, `top_k`, `input` |
| `LlmResponseWrapper` | LLM response interceptor - fields: `response_type`, `content`, `tool_name`, `tool_args`, `tool_id` |
| `ToolCallWrapper` | Tool call interceptor - fields: `name`, `args`, `id` |
| `ToolResultWrapper` | Tool result interceptor - fields: `result`, `success`, `error` |

---

## Token Usage

#### TokenUsage

| Property | Type | Description |
|----------|------|-------------|
| `prompt_tokens` | `int` | Prompt token count |
| `completion_tokens` | `int` | Completion token count |
| `total_tokens` | `int` | Total token count |

#### TokenBudgetReport

| Property | Type | Description |
|----------|------|-------------|
| `status` | `BudgetStatus` | Budget status |
| `usage_percentage` | `float` | Usage percentage |
| `token_usage` | `TokenUsage` | Current usage |

#### BudgetStatus

| Status | Description |
|--------|-------------|
| `Normal` | Within budget |
| `Warning` | Approaching limit |
| `Exceeded` | Over budget |
| `Critical` | Critically over budget |

---

## LlmMessage

```python
from nbos import LlmMessage

msg = LlmMessage.system("System prompt")
msg = LlmMessage.user("User message")
msg = LlmMessage.assistant("Assistant response")
msg = LlmMessage.assistant_tool_call("tool_name", "tool_call_id", {"arg": "value"})
msg = LlmMessage.tool_result("tool_call_id", "result")
```

#### Methods

| Method | Description |
|--------|-------------|
| `to_py()` | Convert to Python dict |
| `from_py(dict)` | Create from dict (static) |

---

## PythonTool

Low-level tool wrapper for Python functions.

```python
from nbos_native import PythonTool

tool = PythonTool(
    name="weather",
    description="Get weather for a city",
    parameters='{"city": {"type": "string"}}',
    schema='{"type": "object", "properties": {"city": {"type": "string"}}}',
    callback=lambda args: '{"temp": 22}',
)
await agent._inner.add_tool(tool)
```

---

## Best Practices

- Use `async with BrainOS()` for automatic bus lifecycle management
- Use `AgentBuilder` fluent API for agent configuration
- Prefer `@tool` decorator for tool definitions
- Both sync and async callbacks are supported — use async for I/O-bound operations (API calls, database queries)
- Async callbacks are auto-detected via `inspect.iscoroutinefunction()` — no special registration needed
- Register global tools via `brain.register_global()` for reuse across agents
- Use `ConfigLoader.discover()` for environment-specific configuration
- Keep one `Bus` instance per process
- Use `session.save_full()` / `session.restore_full()` for conversation persistence
- Hooks and plugins can also be async — use for rate limiting, logging, request/response enrichment

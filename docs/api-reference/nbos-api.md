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
| `create(**options)` | Construct and start in one call (classmethod) | `BrainOS` |
| `start()` | Create the bus and enter the started state (idempotent) | `BrainOS` |
| `stop()` | Close the bus and return to the stopped state | `None` |
| `agent(name, **options)` | Create an AgentBuilder | `AgentBuilder` |
| `create_bus(**options)` | Build a standalone `BusManager` | `BusManager` |
| `register_global(*tools)` | Register tools available to all agents | `BrainOS` |
| `tools(*tools)` | Alias for `register_global` | `BrainOS` |

#### Properties

| Property | Type | Description |
|----------|------|-------------|
| `bus` | `Bus` | The underlying Bus instance |
| `is_started` | `bool` | Whether `start()` has run and `stop()` has not |
| `config` | `Config` | The loaded configuration |
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
| `with_system(prompt)` | Alias for with_prompt | `AgentBuilder` |
| `with_temperature(temp)` | Set temperature | `AgentBuilder` |
| `with_max_tokens(tokens)` | Set max tokens | `AgentBuilder` |
| `with_timeout(secs)` | Set timeout | `AgentBuilder` |
| `with_api_mode(mode)` | Set the API mode (`chat` or `responses`) | `AgentBuilder` |
| `with_reasoning_effort(effort)` | Set reasoning effort for reasoning models | `AgentBuilder` |
| `with_circuit_breaker(max_failures, cooldown_secs)` | Configure the circuit breaker (matches JS `circuitBreaker`) | `AgentBuilder` |
| `with_rate_limit(capacity, window_secs, max_retries)` | Configure the rate limiter (matches JS `rateLimit`) | `AgentBuilder` |
| `with_config(config)` | Apply a config mapping in one call (matches JS `withConfig`) | `AgentBuilder` |
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
| `with_bash(name, workspace_root, jobs=None, session=None)` | Add bash tool, optionally routing background jobs | `AgentBuilder` |
| `with_memory(memory, limit=5)` | Recall up to `limit` items from a `Memory` store on each text run | `AgentBuilder` |
| `with_max_steps(steps)` | Set the maximum ReAct steps | `AgentBuilder` |
| `with_persona_complete(complete=True)` | Use the system prompt as the whole prompt | `AgentBuilder` |
| `with_compaction_input_tokens(tokens)` | Cap one compaction request's input | `AgentBuilder` |
| `with_tool_output_cap(bytes_cap)` | Cap the text one tool result may add to the context | `AgentBuilder` |
| `with_parallel_tool_calls(enabled=True)` | Run one response's tool calls concurrently | `AgentBuilder` |
| `with_web_search(enabled=True)` | Offer the provider's hosted web-search tool | `AgentBuilder` |
| `with_context_budget(tokens)` | Set the send-side history ceiling for mid-turn compaction | `AgentBuilder` |
| `with_skill_catalogue(enabled=True)` | Advertise skills as a catalogue instead of searching | `AgentBuilder` |
| `with_fallback(base_url, model, api_key="")` | Append an alternate endpoint tried after a provider failure | `AgentBuilder` |
| `with_plan_tool(enabled=True)` | Register the `update_plan` tool at start | `AgentBuilder` |
| `defer_tool(name)` | Keep a tool out of the advertised array; it stays callable | `AgentBuilder` |
| `defer_tool_prefix(prefix)` | Keep every tool whose name starts with `prefix` out of the array | `AgentBuilder` |

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
| `with_memory(memory, limit=5)` | Attach a store that recalls up to `limit` items per text run | `Agent` |
| `chat(message)` | Alias for `ask` | `str` |
| `react(task)` | Run with ReAct reasoning | `str` |
| `stream(task)` | Stream response tokens | `AsyncIterator` |
| `reset_metrics()` | Reset the collected performance metrics | `Agent` |
| `plan_items()` | Snapshot the working plan as `{text, status}` items | `list[dict]` |
| `set_plan(items)` | Replace the working plan; an empty list clears it | `Agent` |
| `clear_plan()` | Forget the whole working plan | `Agent` |
| `add_plan_tool()` | Register the `update_plan` tool | `bool` |
| `steer(content, ticket)` | Queue a message for the running turn's next step | `bool` |
| `steer_depth()` | Number of steered messages waiting | `int` |
| `take_undelivered_steers()` | Take back steered messages no step delivered | `list[int]` |
| `defer_tool(name)` | Keep a tool out of the advertised array; it stays callable | `Agent` |
| `defer_tool_prefix(prefix)` | Keep every tool whose name starts with `prefix` out of the array | `Agent` |
| `is_deferred_tool(name)` | Whether a tool is withheld from the advertised array | `bool` |
| `has_deferred_tools()` | Whether any registered tool is withheld | `bool` |
| `remove_tools(names)` | Remove asynchronous tools by name | `int` |
| `add_live_prompt_section(section)` | Append a live extension prompt fragment | `Agent` |
| `remove_live_prompt_section(section)` | Remove one live extension prompt fragment | `bool` |
| `register_skill(name, description, content, path=None)` | Register an inline skill | `Agent` |
| `remove_skills_by_names(names)` | Remove skills by name | `int` |
| `list_skills()` | Snapshot every registered skill | `list[dict]` |
| `context_budget(tokens=None)` | Read or set the send-side context budget | `int \| Agent` |
| `set_context_budget(tokens)` | Set the send-side context budget | `Agent` |
| `skill_catalogue(enabled=None)` | Read or choose catalogue vs on-demand search | `bool \| Agent` |
| `set_skill_catalogue(enabled)` | Choose catalogue vs on-demand search | `Agent` |
| `add_goal_tools(store)` | Register `get_goal`/`update_goal` against a `GoalStore` | `int` |
| `add_job_tools(jobs, session="default")` | Register `job_output`/`job_kill` against a `JobRegistry` | `int` |
| `add_bash_tool(name="bash", workspace_root=None, jobs=None, session=None)` | Register a sandboxed bash tool | `Agent` |

#### Properties

| Property | Type | Description |
|----------|------|-------------|
| `session` | `SessionManager` | Session management |
| `tools` | `list[str]` | Registered tool names |
| `tool_names` | `list[str]` | Alias for `tools` (matches JS `toolNames`) |
| `config` | `dict` | Agent configuration |
| `is_running` | `bool` | Whether an agent call is in flight |
| `metrics` | `dict` | Performance metrics across LLM calls, incl. `min_wall_time_us`/`max_wall_time_us` |

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
| `clear()` | Drop every non-system message and reset the context | `SessionManager` |
| `get_messages()` | Get all messages | `list[dict]` |
| `add_message(role, content)` | Add a message | `SessionManager` |
| `export()` | Export the session snapshot | `dict` |
| `export_json()` | Export the session snapshot as a JSON string | `str` |
| `import_session(data)` | Import a snapshot dict or JSON string | `SessionManager` |
| `set_context(context)` | Replace the opaque session context | `SessionManager` |
| `clear_context()` | Reset the session context to null | `SessionManager` |

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
| `unregister(name)` | Alias for `remove` | `ToolRegistry` |
| `get(name)` | Get tool by name | `ToolDef \| None` |
| `has(name)` | Check if tool exists | `bool` |
| `list()` | List tool names | `list[str]` |
| `list_tools()` | List tool definitions | `list[ToolDef]` |
| `list_tool_defs()` | Alias for `list_tools` | `list[ToolDef]` |
| `list_by_category(category)` | Tools whose category attribute matches | `list[ToolDef]` |
| `filter(predicate)` | New registry with tools the predicate accepts | `ToolRegistry` |
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

#### Agent Behaviour Fields

| Field | Default | Description |
|-------|---------|-------------|
| `persona_complete` | `False` | Treat `system_prompt` as the whole prompt (no appended guidance) |
| `max_steps` | 10 | Maximum ReAct steps before a run is stopped |
| `compaction_input_tokens` | 24000 | Token ceiling for one compaction request |
| `tool_output_cap` | 0 | Bytes one tool result may add (0 keeps the engine default) |
| `parallel_tool_calls` | `False` | Run one response's tool calls concurrently |
| `web_search` | `False` | Offer the provider's hosted web-search tool (Responses only) |
| `context_budget` | `None` | Send-side history ceiling for mid-turn compaction |
| `skill_catalogue` | `None` | Advertise skills as a catalogue instead of searching |
| `fallbacks` | `[]` | Alternates added with `add_fallback(base_url, model, api_key="")` |

`AgentConfig.fallback_list()` returns the configured alternates as plain
dictionaries; `clear_fallbacks()` drops them.

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
| `add_bash_tool(name, workspace_root, jobs=None, session=None)` | Add bash tool, optionally routing background jobs |
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

The `*Wrapper` aliases name the high-level wrappers returned by `BusManager`
(`PublisherWrapper`, `SubscriberWrapper`, `QueryWrapper`, `QueryableWrapper`,
`CallerWrapper`, `CallableWrapper`). The plain `Publisher`, `Subscriber`,
`Query`, `Queryable`, `Caller` and `Callable` exports are the lower-level
native classes, kept under their original names for compatibility.

#### BusManager

High-level bus lifecycle and factory, mirroring the JavaScript `BusManager`.

```python
from nbos import BusManager

bus = await BusManager.create(mode="peer")   # construct + start
async with BusManager() as bus:              # or use it as a context manager
    ...
```

| Method | Description | Returns |
|--------|-------------|---------|
| `create(**options)` | Construct and start (classmethod) | `BusManager` |
| `start()` | Create the underlying bus if needed | `BusManager` |
| `stop()` | Close the underlying bus | `None` |
| `mode(mode)` | Set the mode (fluent) | `BusManager` |
| `connect(addresses)` | Set connect endpoints (fluent) | `BusManager` |
| `listen(addresses)` | Set listen endpoints (fluent) | `BusManager` |
| `peer(id)` | Set the peer id (fluent) | `BusManager` |
| `publish(topic, payload, is_json=False)` | Publish text or JSON | `None` |
| `publisher(topic)` | Create a `Publisher` | `Publisher` |
| `subscriber(topic)` | Create a `Subscriber` | `Subscriber` |
| `query(topic)` | Create a `Query` | `Query` |
| `queryable(topic, handler=None)` | Create a `Queryable` | `Queryable` |
| `caller(name)` | Create a `Caller` | `Caller` |
| `callable(uri, handler=None)` | Create a `Callable` | `Callable` |
| `bus` | The underlying native `Bus` (property) | `Bus` |

`create_publisher`/`create_subscriber`/`create_query`/`create_queryable`/
`create_caller`/`create_callable` and `publish_text`/`publish_json` remain as
aliases of the fluent names above.

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
| `publish(payload, is_json=False)` | Publish text or JSON |
| `publish_text(payload)` | Publish text |
| `publish_json(data)` | Publish JSON |
| `topic` | The publisher topic (property) |

#### Subscriber

| Method | Description |
|--------|-------------|
| `recv(timeout_ms=None)` | Receive a message, with an optional timeout |
| `recv_json(timeout_ms=None)` | Receive and decode a JSON message |
| `run(callback)` | Run a callback loop |
| `run_json(callback)` | Run a JSON callback loop |
| `next()` | One async-iterator step: `{"done", "value"}` |
| `topic` | The subscriber topic (property) |

`recv_with_timeout_ms` and `recv_json_with_timeout_ms` remain as deprecated
aliases of `recv` and `recv_json`.

---

## Query / Queryable

#### Query

| Method | Description |
|--------|-------------|
| `ask(payload, timeout_ms=None)` | Send a text query |
| `ask_json(payload, timeout_ms=None)` | Send JSON and decode the JSON response |
| `topic` | The query topic (property) |

`query_text` and `query_text_timeout_ms` remain as deprecated aliases.

#### Queryable

| Method | Description |
|--------|-------------|
| `handle(handler)` | Register the handler, applied on `start` (fluent) |
| `start()` | Start serving |
| `run(handler)` | Set the handler and start serving |
| `run_json(handler)` | Set a JSON handler and start serving |

---

## Caller / Callable

#### Caller

| Method | Description |
|--------|-------------|
| `call(payload)` | Call the remote service |
| `call_json(payload)` | Send JSON and decode the JSON response |

`call_text` remains as a deprecated alias.

#### Callable

| Method | Description |
|--------|-------------|
| `handle(handler)` | Register the handler, applied on `start` (fluent) |
| `start()` | Start serving |
| `run(handler)` | Set the handler and start serving |
| `run_json(handler)` | Set a JSON handler and start serving |

| Property | Type | Description |
|----------|------|-------------|
| `is_started` | `bool` | Whether the callable is serving |

---

## Config

High-level configuration wrapper. Sources are declared fluently, then `load()`
resolves them once and caches the result for `get()` and the accessors.

```python
from nbos import Config

cfg = Config().discover().load()
print(cfg.model, cfg.base_url)

cfg = Config.from_inline({"global_model": {"model": "x"}})
```

| Member | Description |
|--------|-------------|
| `Config(strategy="override", options=None)` | Constructor |
| `from_file(path)` / `from_directory(path)` / `from_inline(data)` | Construct, load and return (classmethods) |
| `file(path)` / `directory(path)` / `inline(data)` | Declare a source (fluent) |
| `discover()` | Add the standard search paths |
| `reset()` | Clear sources and cached state |
| `load()` | Resolve and cache the config |
| `reload()` | Reset, then load again |
| `get(key, default=None)` | Dotted-path lookup, loading on first use |
| `global_model` / `model` / `base_url` / `api_key` / `bus` | Convenience accessors (properties) |
| `to_json()` | The cached config dict |
| `is_loaded()` | Whether `load()` has run |

`add_file`/`add_directory`/`add_inline`/`load_sync`/`reload_sync` remain as
deprecated aliases.

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

## Goal / GoalStore

A goal is the durable objective a session pursues; the plan answers "what steps
are left", the goal answers "what is all of this for". Both classes mirror the
JavaScript binding exactly.

#### Goal

| Method / Property | Description |
|-------------------|-------------|
| `Goal.create(objective, max_rounds=None)` | Create an active, armed goal (blank objectives raise) |
| `Goal.from_json(json)` | Restore a goal from its snapshot |
| `to_json()` | The goal as a plain dictionary |
| `objective` \| `state` \| `rounds` \| `max_rounds` | Durable fields |
| `activation` \| `blocked_reason` | Continuation flag and blocker |
| `remaining` \| `can_advance` \| `exhausted` | Round-cap projections |
| `advance()` \| `pause()` \| `resume()` \| `complete()` | Phase transitions |
| `block(code, message)` | Stop on a lower-kebab-case policy code |
| `arm()` \| `disarm()` \| `sanitize()` | Continuation and bounds |

#### GoalStore

Seed a store with the session's goal, hand it to `Agent.add_goal_tools`, and
apply `take_settle()` when the turn ends.

| Method | Description |
|--------|-------------|
| `GoalStore(goal)` | Seed a store from a `Goal` |
| `snapshot()` | The goal as the model sees it now |
| `complete()` / `block(code, message)` | Record a settle transition |
| `take_settle()` | Take the recorded transition, if any |
| `has_settle()` | Whether a transition still waits for the host |

## JobRegistry

The live roster of background jobs. Share one instance between the bash tool
that spawns jobs, the job tools that read them, and the host that draws the
list.

| Method | Description |
|--------|-------------|
| `JobRegistry(job_cap=None, output_cap=None)` | Create a bounded roster |
| `list(session)` | Every job started by `session`, oldest first |
| `get(id)` | One job's metadata, or `None` |
| `output(id, max_bytes=None)` | Captured output, or `None` |
| `kill(id)` | Stop a running job |
| `wait(id, timeout_ms=0)` | Wait for settlement, then return metadata |
| `running_count(session)` / `settled_count(session)` | Roster counts |
| `clear_settled(session)` | Drop settled jobs |
| `version()` | Counter that changes on every roster change |

## Ferris SDK

Ferris is the BOS extension framework. These classes bind the SDK's authoring
surface: a Python object supplies the same start/invoke/shutdown callbacks a
Rust Extension implements, and FerrisHarness runs it in process exactly like the
Rust TestHarness.

This is the authoring/runtime harness, not the package manager. Installing,
activating and isolating untrusted packages through the Ferris runtimes is a
separate host surface.

### FerrisManifest

| Member | Description |
|--------|-------------|
| `FerrisManifest.parse(toml)` | Parse and validate an extension.toml schema-2.0 document |
| `manifest.id` | The extension id |
| `manifest.runtime_kind` | The runtime kind the manifest asks for |
| `manifest.to_toml()` | The canonical TOML document |
| `manifest.to_json()` | The manifest as a plain dictionary |

### FerrisManifestBuilder

Chainable schema-2.0 manifest builder. Every method returns a new builder.

| Member | Description |
|--------|-------------|
| `FerrisManifestBuilder(id, name, version, description)` | Start a manifest |
| `host(req)` / `extension_api(req)` | Compatibility ranges |
| `runtime(kind)` / `runtime_config(key, value)` | Runtime selection and opaque config |
| `capability(cap)` | Require one capability |
| `contribution(kind, id, payload=None)` | Declare one contribution with an optional payload |
| `author` / `license` / `repository` / `homepage` | Identity metadata |
| `dependency(id, requirement)` | Dependency requirement |
| `provide(spec)` / `require(spec)` | Service declarations from dictionaries |
| `max_memory_mb` / `max_execution_ms` / `max_concurrent_calls` / `max_output_bytes` / `max_table_elements` | Resource ceilings |
| `configuration_schema(path)` / `configuration_defaults(value)` | Configuration block |
| `startup(policy)` | `on-startup`, `on-demand`, `on-workspace-open`, `manual` |
| `build()` | Validate and return a `FerrisManifest` |

```python
manifest = (
    FerrisManifestBuilder("com.example.echo", "Echo", "1.0.0", "Echo")
    .runtime("declarative")
    .capability("agent.tools.register")
    .contribution("tool", "echo", {"description": "Echo"})
    .startup("on-demand")
    .build()
)
```

### FerrisHarness

| Member | Description |
|--------|-------------|
| `FerrisHarness(manifest, root, start, invoke, shutdown=None)` | Build a harness from a TOML manifest and callbacks |
| `harness.start()` | Run start(ctx) and return its contributions |
| `harness.invoke(id, input)` | Invoke a contribution id as a tool |
| `harness.invoke_contribution(kind, id, input)` | Invoke with an explicit kind |
| `harness.shutdown()` | Run shutdown() |
| `harness.manifest()` | The parsed manifest |
| `harness.runtime_kind` / `harness.root` | Runtime kind and package root |

Callbacks receive one dictionary and may be sync or async:

```python
async def start(ctx):
    return [{"kind": "tool", "id": "echo", "payload": {"description": "Echo"}}]

async def invoke(call):
    return call["input"]  # {contribution, input, context}

harness = FerrisHarness(manifest_toml, "/tmp/echo", start, invoke)
contributions = await harness.start()
await harness.invoke("echo", {"text": "hi"})
await harness.shutdown()
```

### FerrisHost

The full Ferris package host. It owns a canonical store, installs packages,
plans them in dependency order, activates the standard runtimes, exposes
contributions as dictionaries, and forwards invocations. Nothing starts until
apply() is called.

| Member | Description |
|--------|-------------|
| `FerrisHost(root, workspace=None, trust=None, policy_mode=None, on_contributions=None, on_revoke=None, process_sandbox=None)` | Open a host; policy mode is `enforce` (default) or `advisory`; `process_sandbox` is retained for API compatibility and no longer changes behavior |
| `host.root` | Canonical store root |
| `host.install(source)` | Install a package tree and return `{id, version}` |
| `host.install_package(source, origin=None)` | Install with `installed`, `development` or `builtin` origin |
| `host.installed()` | Installed `{id, version}` pairs |
| `host.uninstall(id, purge_data=False)` | Remove an extension, optionally purging its data |
| `host.versions(id)` / `host.active_version(id)` | Version list and active version |
| `host.activate(id, version)` / `host.rollback(id)` | Switch the active version |
| `host.set_enabled(id, enabled)` | Enable or disable without uninstalling |
| `host.clear_failure(id)` / `host.quarantine(id, reason)` | Lifecycle recovery |
| `host.package_root(id, version)` | Canonical package directory |
| `host.plan()` | Activation order (dependency first) |
| `host.apply()` | Start every planned package; returns applied contributions |
| `host.reload()` | Unload and re-apply the current plan |
| `host.unload()` | Revoke contributions and shut down every runtime |
| `host.contributions()` | Applied contributions as a dictionary |
| `host.invoke(extension, kind, id, input)` | Invoke one contribution on a started runtime |
| `host.register_extension(manifest, root, start, invoke, shutdown=None)` | Register an in-process Python extension as a linked package |
| `host.subscribe_events(topics, callback)` / `host.unsubscribe_events(id)` | Host event subscription and cancellation |
| `host.publish_event(topic, payload)` | Publish a host event; returns `{delivered, dropped}` |
| `await host.register_agent_tools(agent)` | Register applied tool contributions on an Agent/AgentBuilder |
| `host.grant(extension, capability, scope=None, note=None)` | Persist a capability grant; an unknown scope is an error |
| `host.revoke_grant(extension, capability)` | Remove a grant |
| `host.grants(extension)` | Every grant for one extension |
| `host.audit_degraded()` | First audit persistence failure, or `None` |

Runtimes: declarative, process, MCP, Cordis, Extism and WebAssembly Component.
`builtin` manifests are refused because no host-linked builtin is registered.

```python
from nbos import FerrisHost

host = FerrisHost("/tmp/ferris-store", "/tmp/workspace")
host.install("/path/to/package")
host.plan()
contributions = await host.apply()
result = await host.invoke("com.example.echo", "tool", "echo", {"x": 1})
await host.unload()
```

## Prompt Templates

| Function | Description |
|----------|-------------|
| `render_template(template, variables)` | Substitute `{{name}}` placeholders from a string mapping |
| `with_memory(template, memory=None)` | Place a recalled-memory block, honouring `{{memory}}` |
| `estimate_tokens(text)` | Estimate tokens with the agent's own heuristic |

```python
from nbos import Goal, GoalStore, JobRegistry, render_template

goal = Goal.create("Ship the release")
store = GoalStore(goal)
agent.add_goal_tools(store)

jobs = JobRegistry()
agent.add_job_tools(jobs, session="default")

prompt = render_template("You are {{role}}.", {"role": "a reviewer"})
```

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

## Multimodal Content

`Content`, `ContentPart`, and `Binary` describe a multimodal message so an
agent can send images or audio alongside text. `Content.to_json()` is what the
binding hands to the Rust backend.

### Binary

| Member | Description |
|--------|-------------|
| `Binary.from_base64(content_type, data, name=None)` | Build from base64-encoded bytes |
| `Binary.from_url(content_type, url, name=None)` | Build from a URL |
| `binary.is_image()` | Whether `content_type` starts with `image/` |
| `binary.is_audio()` | Whether `content_type` starts with `audio/` |
| `binary.url()` | The raw URL, or a `data:` URL for base64 sources |
| `binary.to_dict()` | `{ content_type, source: { url or base64 }, name? }` |

### ContentPart

| Member | Description |
|--------|-------------|
| `ContentPart.text(text)` | A text part |
| `ContentPart.binary(content_type, data, name=None)` | A base64 binary part; `bytes` are base64-encoded |
| `ContentPart.binary_url(content_type, url, name=None)` | A URL binary part |
| `ContentPart.image(url, name=None)` | A URL image part typed `image/jpeg` |
| `ContentPart.audio(data, format="mp3")` | A base64 audio part |
| `ContentPart.audio_url(url, format="mp3")` | A URL audio part |
| `part.to_dict()` | `{ type: "text", text }` or `{ type: "binary", binary }` |

### Content

| Member | Description |
|--------|-------------|
| `Content.text(text)` | Plain text content |
| `Content.parts(parts)` | Content from a list of `ContentPart` |
| `Content.image(url, name=None)` | One URL image part typed `image/jpeg` |
| `Content.audio(data, format="mp3")` | One base64 audio part |
| `Content.audio_url(url, format="mp3")` | One URL audio part |
| `content.to_json()` | JSON string of a text object or a parts array |
| `content.is_multimodal()` | Whether the content holds binary parts |

## Memory

`Memory` is an in-process, keyword-ranked store that mirrors the Rust
`agent::memory::InMemoryMemory`. Recall earlier facts and re-inject them
into a prompt.

| Member | Description |
|--------|-------------|
| `Memory()` | Create an empty store |
| `memory.add(content, metadata=None)` | Store text and return the item dict |
| `memory.all()` | Every item, oldest first |
| `memory.search(query, limit=5, metadata=None)` | Up to `limit` relevant items, best first; `metadata` keeps exact key/value matches, a blank query returns the most recent |
| `memory.with_max_items(max_items)` | Cap the store, evicting the oldest items on overflow |
| `memory.max_items` | The configured cap, or `None` |
| `memory.remove(id)` | Remove by id, returning whether it existed |
| `memory.clear()` | Remove every item |
| `memory.len()` | Number of stored items |
| `memory.is_empty()` | Whether the store is empty |
| `memory.to_json()` | JSON array of every item |
| `memory.recall_block(query, limit=5)` | Prompt preamble of the best matches, or `""` |
| `memory.save(path)` | Write every item as JSON lines; returns the count |
| `Memory.load(path)` | New store from a JSON-lines file written by `save` |

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

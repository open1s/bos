# BrainOS Python API User Guide

This guide provides a unified, consistent API for using BrainOS in Python. The API follows a fluent/decorator style for intuitive usage.

## Table of Contents

1. [Installation](#installation)
2. [Quick Start](#quick-start)
3. [Core Concepts](#core-concepts)
4. [Agent API](#agent-api)
5. [Tool Registration](#tool-registration)
6. [Bus Communication](#bus-communication)
7. [Query/Queryable](#queryqueryable)
8. [Caller/Callable](#callercallable)
9. [Configuration](#configuration)
10. [API Reference](#api-reference)

---

## Installation

```bash
pip install brainos
```

Or install from source:

```bash
cd crates/nbos
pip install -e .
```

---

## Quick Start

```python
import asyncio
from nbos import BrainOS, tool

@tool("Add two numbers together")
def add(a: int, b: int) -> int:
    return a + b

async def main():
    async with BrainOS() as brain:
        agent = brain.agent("assistant")
        agent.register(add)
        result = await agent.ask("What is 42 + 58?")
        print(result)

asyncio.run(main())
```

---

## Core Concepts

### BrainOS (Main Entry Point)

The `BrainOS` class is the main entry point that manages the lifecycle:

```python
async with BrainOS() as brain:
    # brain is ready to use
    agent = brain.agent("my-agent")
```

### Agent

An `Agent` represents an LLM-powered agent that can use tools:

```python
agent = brain.agent("assistant")
agent = brain.agent("coder", system_prompt="You are a coding assistant.")
```

### Tool

Tools are functions that the LLM can call. Use the `@tool()` decorator:

```python
@tool("Description of what the tool does")
def function_name(param1: type, param2: type) -> return_type:
    return result
```

---

## Agent API

### Creating an Agent

```python
# Basic agent
agent = brain.agent("assistant")

# Agent with custom config
agent = brain.agent(
    "coder",
    system_prompt="You are a helpful coding assistant.",
    model="gpt-4",
    temperature=0.5,
    timeout_secs=180,
)
```

### Fluent Configuration

Use chainable methods to configure the agent:

```python
agent = brain.agent("assistant") \
    .with_model("gpt-4") \
    .with_temperature(0.3) \
    .with_prompt("You are a math tutor.") \
    .with_timeout(300)
```

### Running the Agent

```python
# Simple Q&A (no tool use)
result = await agent.ask("What is Python?")

# Run with tool use enabled
result = await agent.react("Calculate 2 + 2")

# Simple conversation
result = await agent.chat("Hello!")
result = await agent.run_simple("Hello!")

# Streaming response
async for chunk in await agent.stream("Tell me a story"):
    print(chunk, end="", flush=True)
```

### Registering Tools

```python
# Single tool
agent.register(add)

# Multiple tools
agent.register_many(tool1, tool2, tool3)

# Or chain them
agent.register(tool1).register(tool2)

# Register skills from a directory
agent.register_skills("/path/to/skills_directory")
```

### Loading Skills from Directory

Skills are Python modules containing `@tool` decorated functions. Use `register_skills()` to load all skills from a directory:

```python
# Directory structure:
# /path/to/skills/
#   ├── math.py        # contains @tool decorated functions
#   └── search.py      # contains @tool decorated functions

agent.register_skills("/path/to/skills")
```

When `start()` is called, all skills in the directory are automatically registered.

---

## Tool Registration

### Using the `@tool()` Decorator

```python
from nbos import tool

@tool("Calculate a math expression")
def calc(expression: str) -> str:
    result = eval(expression)
    return str(result)

@tool("Get weather information")
def get_weather(city: str) -> dict:
    return {"city": city, "temperature": 22, "unit": "celsius"}
```

### Custom Schema

```python
@tool("Calculate", schema={
    "type": "object",
    "properties": {
        "expression": {
            "type": "string",
            "description": "Math expression to evaluate"
        }
    },
    "required": ["expression"]
})
def calc(expression: str) -> str:
    return str(eval(expression))
```

### Manual ToolDef

```python
from nbos.tool import ToolDef

def my_handler(args: dict) -> str:
    return f"Processed: {args}"

tool_def = ToolDef(
    name="my_tool",
    description="A custom tool",
    callback=my_handler,
    parameters={"arg1": {"type": "string"}},
    schema={"type": "object", "properties": {"arg1": {"type": "string"}}}
)

agent.register(tool_def)
```

---

## Bus Communication

The Bus provides pub/sub messaging between components.

### Using BusManager

```python
from nbos import BusManager

async with BusManager() as bus:
    # Publish
    await bus.publish_text("my/topic", "hello")
    await bus.publish_json("my/topic", {"data": 123})

    # Create publisher
    pub = await bus.create_publisher("output/topic")
    await pub.publish_text("message")

    # Create subscriber
    sub = await bus.create_subscriber("input/topic")
    msg = await sub.recv()
```

### Subscriber Patterns

```python
sub = await bus.create_subscriber("my/topic")

# One-shot receive
msg = await sub.recv()
msg = await sub.recv_with_timeout_ms(5000)

# Get JSON
data = await sub.recv_json_with_timeout_ms(5000)

# Callback loop
await sub.run(lambda msg: print(f"Received: {msg}"))

# Async iteration
async for msg in sub:
    print(msg)
```

---

## Query/Queryable

Request-response pattern with timeout support.

### Server Side (Queryable)

```python
def upper_handler(text: str) -> str:
    return text.upper()

async with BusManager() as bus:
    q = await bus.create_queryable("svc/upper", upper_handler)
    await q.start()
```

### Client Side (Query)

```python
async with BusManager() as bus:
    query = await bus.create_query("svc/upper")
    result = await query.query_text("hello")  # "HELLO"
    result = await query.query_text_timeout_ms("hello", 5000)  # with timeout
```

### Query API

**Properties:**
| Property | Type | Description |
|-----------|------|-------------|
| `topic` | `str` | Topic name |

**Methods:**
| Method | Description |
|--------|-------------|
| `query_text(payload)` | Send text query |
| `query_text_timeout_ms(payload, ms)` | Send with timeout |

### Queryable API

**Methods:**
| Method | Description |
|--------|-------------|
| `start()` | Start server |
| `run(handler)` | Run with handler |
| `run_json(handler)` | Run JSON handler |

---

## Caller/Callable

RPC-style request-response pattern.

### Server Side (Callable)

```python
def echo_handler(text: str) -> str:
    return f"echo: {text}"

async with BusManager() as bus:
    srv = await bus.create_callable("svc/echo", echo_handler)
    await srv.start()
```

### Client Side (Caller)

```python
async with BusManager() as bus:
    caller = await bus.create_caller("svc/echo")
    result = await caller.call_text("ping")  # "echo: ping"
```

### Caller API

**Methods:**
| Method | Description |
|--------|-------------|
| `call_text(payload)` | Call remote service |

### Callable API

**Properties:**
| Property | Type | Description |
|-----------|------|-------------|
| `is_started` | `bool` | Whether server is running |

**Methods:**
| Method | Description |
|--------|-------------|
| `start()` | Start server |
| `run(handler)` | Run with handler |
| `run_json(handler)` | Run JSON handler |

---

## Configuration

### Using Config Files

BrainOS looks for config in:
- `~/.bos/conf/config.toml`
- `./conf/config.toml`
- Environment variables

Example config:
```toml
[global_model]
api_key = "your-api-key"
base_url = "https://api.openai.com/v1"
model = "gpt-4"
```

### Using Config Class

```python
from nbos import Config

config = Config() \
    .discover() \
    .add_file("/path/to/config.toml") \
    .add_inline({"key": "value"})

data = config.load_sync()
```

### Environment Variables

- `BOS_API_KEY` - API key for LLM
- `BOS_BASE_URL` - Base URL for LLM API
- `BOS_MODEL` - Model name

---

## Hooks, Plugins, and Sessions

### Hooks

Hooks allow you to intercept and react to events during agent execution.

#### Using Hooks

```python
from nbos import BrainOS, HookEvent
import asyncio

async def main():
    async with BrainOS() as brain:
        agent = brain.agent("assistant")
        
        # Register hooks
        agent.hooks().register(HookEvent.BeforeToolCall, lambda ctx: {
            "tool_name": ctx.get("tool_name"),
            "decision": "continue"  # or "abort" or {"error": "message"}
        })
        
        agent.hooks().register(HookEvent.AfterToolCall, lambda ctx: {
            "tool_name": ctx.get("tool_name"),
            "tool_result": ctx.get("tool_result"),
            "decision": "continue"
        })
        
        agent.hooks().register(HookEvent.BeforeLlmCall, lambda ctx: {
            "prompt": ctx.get("prompt"),
            "decision": "continue"
        })
        
        agent.hooks().register(HookEvent.AfterLlmCall, lambda ctx: {
            "response": ctx.get("response"),
            "decision": "continue"
        })
        
        agent.hooks().register(HookEvent.OnError, lambda ctx: {
            "error": ctx.get("error"),
            "decision": "continue"
        })
        
        result = await agent.react("Say hello!")
        print(result)

asyncio.run(main())
```

#### Hook Events

| Event | Description |
|-------|-------------|
| `BeforeToolCall` | Fired before a tool is called |
| `AfterToolCall` | Fired after a tool completes |
| `BeforeLlmCall` | Fired before LLM API call |
| `AfterLlmCall` | Fired after LLM API call |
| `OnMessage` | Fired for each message |
| `OnComplete` | Fired when agent completes |
| `OnError` | Fired when an error occurs |

#### Hook Decisions

Return a dict to control execution:

| Decision | Description |
|----------|-------------|
| `{"decision": "continue"}` | Proceed normally (default) |
| `{"decision": "abort"}` | Abort current operation |
| `{"decision": "error", "message": "error message"}` | Return an error |

### Plugins

Plugins allow you to preprocess and postprocess LLM requests and responses.

#### Using Plugins

```python
from nbos import BrainOS
from nbos.plugin import AgentPlugin
import asyncio

class MyPlugin(AgentPlugin):
    async def process_llm_request(self, wrapper):
        # Modify request before sending to LLM
        # Example: add system prompt prefix
        request = wrapper.into_request()
        # Modify request here
        return wrapper.__class__(request)
    
    async def process_llm_response(self, wrapper):
        # Modify response after receiving from LLM
        response = wrapper.into_response()
        # Modify response here
        return wrapper.__class__(response)

async def main():
    async with BrainOS() as brain:
        agent = brain.agent("assistant")
        
        # Register plugin
        agent.plugins().register_blocking(MyPlugin())
        
        result = await agent.react("Say hello!")
        print(result)

asyncio.run(main())
```

### Session Management

BrainOS provides session management for persisting agent state across restarts.

#### Session Operations

```python
from nbos import BrainOS
import asyncio

async def main():
    async with BrainOS() as brain:
        agent = brain.agent("assistant")
        
        # Save session
        agent.save_message_log("/tmp/session.json")
        
        # Later, restore session
        # agent.restore_message_log("/tmp/session.json")
        
        result = await agent.react("Say hello!")
        print(result)

asyncio.run(main())
```

#### Session Info Methods

| Method | Description |
|--------|-------------|
| `add_message(message)` | Add message to conversation log |
| `get_messages()` | Get conversation messages |
| `save_message_log(path)` | Save message log to file |
| `restore_message_log(path)` | Restore message log from file |

---

## API Reference

For the complete API reference, see [Python API Reference](./api-reference/nbos-api.md).

### `BrainOS`

Main entry point for BrainOS.

**Constructor:**
```python
BrainOS(api_key=None, base_url=None, model=None)
```

**Attributes:**
| Attribute | Type | Description |
|-----------|------|-------------|
| `__version__` | `str` | BrainOS package version (e.g., "1.2.0") |

**Methods:**
| Method | Description |
|--------|-------------|
| `agent(name, **options)` | Create a new agent |
| `bus` | Get the underlying Bus |

### `Agent`

LLM-powered agent with tool support.

**Constructor:**
```python
Agent(bus, name="assistant", model=..., base_url=..., api_key=..., system_prompt=..., temperature=0.7, timeout_secs=120)
```

**Methods:**
| Method | Description | Returns |
|--------|-------------|---------|
| `with_model(model)` | Set model | `Agent` |
| `with_prompt(prompt)` | Set system prompt | `Agent` |
| `with_temperature(temp)` | Set temperature | `Agent` |
| `with_timeout(secs)` | Set timeout | `Agent` |
| `with_resilience(**opts)` | Set resilience config | `Agent` |
| `register(tool)` | Register a tool | `Agent` |
| `register_many(*tools)` | Register multiple tools | `Agent` |
| `start()` | Initialize agent | `Agent` |
| `ask(question)` | Run agent | `str` |
| `chat(message)` | Simple chat | `str` |
| `run_simple(message)` | Simple run | `str` |
| `react(task)` | Run with ReAct | `str` | 
| `stream(task)` | Stream response | `AsyncIterator` |

### Resilience Configuration

The Agent supports configuring circuit breaker and rate limiter for resilience via three methods:

**Method 1: Fluent API (`with_resilience()`)**
```python
from nbos import BrainOS, tool

@tool("Add two numbers")
def add(a: int, b: int) -> int:
    return a + b

async with BrainOS(api_key="sk-...") as brain:
    agent = brain.agent("assistant").register(add)
    agent.with_resilience(
        rate_limit_capacity=40,           # max requests per window
        rate_limit_window_secs=60,         # window duration in seconds
        rate_limit_max_retries=3,          # retry attempts on 429 errors
        rate_limit_retry_backoff_secs=1,      # backoff between retries
        rate_limit_auto_wait=True,            # auto-wait when rate limited
        circuit_breaker_max_failures=5,       # failures before opening circuit
        circuit_breaker_cooldown_secs=30,     # seconds before attempting recovery
    )
    result = await agent.ask("What is 42 + 58?")
```

**Method 2: Constructor kwargs**
```python
async with BrainOS(api_key="sk-...") as brain:
    agent = brain.agent(
        "assistant",
        rate_limit_capacity=40,
        rate_limit_window_secs=60,
        rate_limit_max_retries=3,
        rate_limit_auto_wait=True,
        circuit_breaker_max_failures=5,
        circuit_breaker_cooldown_secs=30,
    )
    result = await agent.ask("What is 42 + 58?")
```

**Method 3: `BrainOS.agent()` with resilience params**
```python
async with BrainOS(api_key="sk-...") as brain:
    agent = brain.agent(
        "assistant",
        rate_limit_capacity=20,
        circuit_breaker_max_failures=5,
    )
    result = await agent.ask("What is 42 + 58?")
```

**Method 4: Using nbos directly with AgentConfig**

```python
from nbos import Agent, AgentConfig

cfg = AgentConfig(
    name="assistant",
    model="gpt-4",
    api_key="sk-...",
    base_url="https://api.openai.com/v1",
    # Circuit Breaker - prevents cascading failures
    circuit_breaker_max_failures=5,      # failures before opening circuit
    circuit_breaker_cooldown_secs=30,      # seconds before attempting recovery
    # Rate Limiter - prevents 429 errors
    rate_limit_capacity=40,               # max requests per window
    rate_limit_window_secs=60,             # window duration in seconds
    rate_limit_max_retries=3,            # retry attempts on rate limit
    rate_limit_retry_backoff_secs=1,    # backoff between retries
    rate_limit_auto_wait=True,             # auto-wait when rate limited
)

# Create agent from config
agent = Agent(cfg)
```

**Circuit Breaker Options:**
| Option | Default | Description |
|--------|---------|-------------|
| `circuit_breaker_max_failures` | 5 | Failures before opening circuit |
| `circuit_breaker_cooldown_secs` | 30 | Seconds before half-open state |

**Rate Limiter Options:**
| Option | Default | Description |
|--------|---------|-------------|
| `rate_limit_capacity` | 40 | Max requests per window |
| `rate_limit_window_secs` | 60 | Window duration in seconds |
| `rate_limit_max_retries` | 3 | Retry attempts on 429 errors |
| `rate_limit_retry_backoff_secs` | 1 | Initial backoff duration |
| `rate_limit_auto_wait` | true | Auto-wait when rate limited |

**Properties:**
| Property | Type | Description |
|-----------|------|-------------|
| `tools` | `list[str]` | Registered tool names |
| `config` | `dict` | Agent configuration |

### `@tool()`

Decorator to create a tool from a function.

**Signature:**
```python
@tool(description: str, *, name: str = None, schema: dict = None)
```

### `BusManager`

Async context manager for Bus lifecycle.

**Constructor:**
```python
BusManager(mode="peer", connect=None, listen=None, peer=None)
```

**Methods:**
| Method | Description |
|--------|-------------|
| `publish_text(topic, payload)` | Publish text message |
| `publish_json(topic, data)` | Publish JSON message |
| `createPublisher(topic)` | Create a publisher |
| `createSubscriber(topic)` | Create a subscriber |
| `createQuery(topic)` | Create a query client |
| `createQueryable(topic, handler)` | Create a queryable server |
| `createCaller(name)` | Create a caller client |
| `createCallable(uri, handler)` | Create a callable server |

### `Publisher`

Message publisher for a specific topic.

**Properties:**
| Property | Type | Description |
|-----------|------|-------------|
| `topic` | `str` | Topic name |

**Methods:**
| Method | Description |
|--------|-------------|
| `publish_text(payload)` | Publish text |
| `publish_json(data)` | Publish JSON |

### `Subscriber`

Message subscriber with receive methods.

**Properties:**
| Property | Type | Description |
|-----------|------|-------------|
| `topic` | `str` | Topic name |

**Methods:**
| Method | Description |
|--------|-------------|
| `recv()` | Receive message (blocking) |
| `recv_with_timeout_ms(ms)` | Receive with timeout |
| `recv_json_with_timeout_ms(ms)` | Receive JSON with timeout |
| `run(callback)` | Run callback loop |
| `run_json(callback)` | Run JSON callback loop |

### `Query` / `Queryable`

Request-response pattern.

**Query Methods:**
| Method | Description |
|--------|-------------|
| `query_text(payload)` | Send query |
| `query_text_timeout_ms(payload, ms)` | Send with timeout |

**Queryable Methods:**
| Method | Description |
|--------|-------------|
| `start()` | Start server |
| `run(handler)` | Run with handler |
| `run_json(handler)` | Run JSON handler |

### `Caller` / `Callable`

RPC pattern.

**Caller Methods:**
| Method | Description |
|--------|-------------|
| `call_text(payload)` | Call remote service |

**Callable Methods:**
| Method | Description |
|--------|-------------|
| `start()` | Start server |
| `run(handler)` | Run with handler |
| `run_json(handler)` | Run JSON handler |
| `is_started` | Check if running |

### `Config`

Configuration loader.

**Methods:**
| Method | Description |
|--------|-------------|
| `discover()` | Auto-discover config files |
| `add_file(path)` | Add config file |
| `add_directory(path)` | Add config directory |
| `add_inline(data)` | Add inline config |
| `reset()` | Reset config |
| `load_sync()` | Load configuration |
| `reload_sync()` | Reload configuration |

---

## Examples

### Complete Example with Tools

```python
import asyncio
from nbos import BrainOS, tool

@tool("Add two numbers")
def add(a: int, b: int) -> int:
    return a + b

@tool("Multiply two numbers")
def multiply(a: int, b: int) -> int:
    return a * b

@tool("Get current time")
def get_time() -> dict:
    from datetime import datetime
    return {"utc": datetime.utcnow().isoformat()}

async def main():
    async with BrainOS() as brain:
        agent = brain.agent("assistant") \
            .register(add) \
            .register(multiply) \
            .register(get_time)
        
        # Ask with tool use
        result = await agent.react("What is 5 + 3? What is 4 * 7?")
        print(result)

asyncio.run(main())
```

### Pub/Sub Example

```python
import asyncio
from nbos import BusManager

async def publisher():
    async with BusManager() as bus:
        await bus.publish_text("events/start", "Hello subscribers!")

async def subscriber():
    async with BusManager() as bus:
        sub = await bus.create_subscriber("events/start")
        msg = await sub.recv_with_timeout_ms(5000)
        print(f"Received: {msg}")

# Run both in separate processes or tasks
```

### Query/Response Example

```python
import asyncio
from nbos import BusManager

def uppercase(text: str) -> str:
    return text.upper()

async def main():
    async with BusManager() as bus:
        # Server
        q = await bus.create_queryable("svc/uppercase", uppercase)
        await q.start()
        
        # Client
        query = await bus.create_query("svc/uppercase")
        result = await query.query_text("hello world")
        print(result)  # "HELLO WORLD"

asyncio.run(main())
```

---

## Error Handling

```python
import asyncio
from nbos import BrainOS

async def main():
    try:
        async with BrainOS() as brain:
            agent = brain.agent("assistant")
            result = await agent.ask("Hello")
    except RuntimeError as e:
        print(f"Runtime error: {e}")
    except Exception as e:
        print(f"Error: {e}")

asyncio.run(main())
```

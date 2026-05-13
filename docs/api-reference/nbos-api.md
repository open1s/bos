# BrainOS Python API Reference

This document provides the complete API reference for the BrainOS Python bindings (`brainos` package).

## Main Entry Point

### BrainOS

Main entry point for BrainOS functionality.

#### Constructor

```python
BrainOS(api_key=None, base_url=None, model=None)
```

Parameters:
- `api_key` (str, optional): API key for LLM provider
- `base_url` (str, optional): Base URL for LLM API
- `model` (str, optional): Model name to use

#### Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `agent(name, **options)` | Create a new agent | `Agent` |
| `bus` | Get the underlying Bus | `Bus` |

#### Properties

| Property | Type | Description |
|----------|------|-------------|
| `bus` | `Bus` | The underlying Bus instance |

#### Example

```python
from nbos import BrainOS

brain = BrainOS(
    api_key="sk-...",
    base_url="https://api.openai.com/v1",
    model="gpt-4"
)

async with brain:
    # ... use brain ...
    pass
```

---

## Agent

LLM-powered agent with tool support.

#### Constructor

```python
Agent(bus, name="assistant", model=..., base_url=..., api_key=..., system_prompt=..., temperature=0.7, timeout_secs=120)
```

Parameters:
- `bus` (Bus): The bus instance
- `name` (str): Agent name
- `model` (str): Model name (default: "gpt-4")
- `base_url` (str): Base URL for LLM API
- `api_key` (str): API key for LLM
- `system_prompt` (str): System prompt for the agent
- `temperature` (float): Temperature for sampling (default: 0.7)
- `timeout_secs` (int): Timeout in seconds (default: 120)

#### Fluent Configuration Methods

Use chainable methods to configure the agent:

| Method | Description | Returns |
|--------|-------------|---------|
| `with_model(model)` | Set model | `Agent` |
| `with_prompt(prompt)` | Set system prompt | `Agent` |
| `with_temperature(temp)` | Set temperature | `Agent` |
| `with_timeout(secs)` | Set timeout | `Agent` |
| `register(tool)` | Register a tool | `Agent` |
| `register_many(*tools)` | Register multiple tools | `Agent` |

#### Agent Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `start()` | Initialize agent | `Agent` |
| `ask(question)` | Run agent with ReAct reasoning | `str` |
| `chat(message)` | Simple chat | `str` |
| `run_simple(message)` | Simple run (no tool use) | `str` |
| `react(task)` | Run with ReAct reasoning | `str` |
| `stream(task)` | Stream response tokens | `AsyncIterator[str]` |

#### Properties

| Property | Type | Description |
|----------|------|-------------|
| `tools` | `list[str]` | Registered tool names |
| `config` | `dict` | Agent configuration |

#### Token Usage

The Agent provides methods to monitor token consumption:

| Method | Description | Returns |
|--------|-------------|---------|
| `token_usage()` | Get current token usage statistics | `TokenUsage` |
| `token_budget_report()` | Get detailed token budget report with status | `TokenBudgetReport` |

#### Classes

| Class | Description |
|-------|-------------|
| `TokenUsage` | Token usage statistics (prompt, completion, total) |
| `TokenBudgetReport` | Budget report with status and usage percentage |
| `BudgetStatus` | Enum: Normal, Warning, Exceeded, Critical |

#### Resilience Configuration

The Agent supports configuring circuit breaker and rate limiter for resilience:

```python
from nbos import Agent, AgentConfig
from react import CircuitBreakerConfig, RateLimiterConfig

cfg = AgentConfig(
    name="assistant",
    model="gpt-4",
    api_key="sk-...",
    base_url="https://api.openai.com/v1",
    circuit_breaker=CircuitBreakerConfig(
        max_failures=5,
        cooldown_secs=30,
    ),
    rate_limit=RateLimiterConfig(
        capacity=40,
        window_secs=60,
        max_retries=3,
        retry_backoff_secs=1,
        auto_wait=True,
    )
)
agent = Agent(cfg)  # Pass config directly to constructor
```

#### Circuit Breaker Options

| Option | Default | Description |
|--------|---------|-------------|
| `circuit_breaker_max_failures` | 5 | Failures before opening circuit |
| `circuit_breaker_cooldown_secs` | 30 | Seconds before half-open state |

#### Rate Limiter Options

| Option | Default | Description |
|--------|---------|-------------|
| `rate_limit_capacity` | 40 | Max requests per window |
| `rate_limit_window_secs` | 60 | Window duration in seconds |
| `rate_limit_max_retries` | 3 | Retry attempts on 429 errors |
| `rate_limit_retry_backoff_secs` | 1 | Initial backoff duration |
| `rate_limit_auto_wait` | true | Auto-wait when rate limited |

#### Example

```python
from nbos import BrainOS, tool
import asyncio

@tool("Add two numbers")
def add(a: int, b: int) -> int:
    return a + b

@tool("Get current time")
def get_time() -> dict:
    from datetime import datetime
    return {"utc": datetime.utcnow().isoformat()}

async def main():
    async with BrainOS() as brain:
        agent = brain.agent("assistant") \
            .register(add) \
            .register(get_time)
        
        # Ask with tool use
        result = await agent.react("What is 5 + 3? What is the current time?")
        print(result)

asyncio.run(main())
```

---

## @tool()

Decorator to create a tool from a function.

#### Signature

```python
@tool(description: str, *, name: str = None, schema: dict = None)
```

Parameters:
- `description` (str): Description of what the tool does
- `name` (str, optional): Tool name (defaults to function name)
- `schema` (dict, optional): JSON Schema for tool parameters

#### Usage

```python
from nbos import tool

@tool("Calculate a math expression")
def calc(expression: str) -> str:
    result = eval(expression)  # In practice, use a safe evaluator
    return str(result)

@tool("Get weather information", name="weather")
def get_weather(city: str) -> dict:
    return {"city": city, "temperature": 22, "unit": "celsius"}

# With custom schema
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

#### Manual Tool Creation

For more control, create a `ToolDef` manually:

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

## BusManager

Async context manager for Bus lifecycle.

#### Constructor

```python
BusManager(mode="peer", connect=None, listen=None, peer=None)
```

Parameters:
- `mode` (str): Bus mode ('peer', 'client', 'server')
- `connect` (str, optional): Connection address for client mode
- `listen` (str, optional): Listen address for server mode
- `peer` (str, optional): Peer address for peer-to-peer mode

#### Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `publish_text(topic, payload)` | Publish text message | `None` |
| `publish_json(topic, data)` | Publish JSON message | `None` |
| `create_publisher(topic)` | Create a publisher | `Publisher` |
| `create_subscriber(topic)` | Create a subscriber | `Subscriber` |
| `create_query(topic)` | Create a query client | `Query` |
| `create_queryable(topic, handler)` | Create a queryable server | `Queryable` |
| `create_caller(name)` | Create a caller client | `Caller` |
| `create_callable(uri, handler)` | Create a callable server | `Callable` |

#### Example

```python
from nbos import BusManager
import asyncio

async def main():
    async with BusManager() as bus:
        # Publish messages
        await bus.publish_text("my/topic", "hello")
        await bus.publish_json("my/topic", {"data": 123})
        
        # Create publisher/subscriber
        pub = await bus.create_publisher("output/topic")
        sub = await bus.create_subscriber("input/topic")
        
        msg = await sub.recv()
        print(f"Received: {msg}")

asyncio.run(main())
```

---

## Publisher

Message publisher for a specific topic.

#### Properties

| Property | Type | Description |
|----------|------|-------------|
| `topic` | `str` | Topic name |

#### Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `publish_text(payload)` | Publish text message | `None` |
| `publish_json(data)` | Publish JSON message | `None` |

---

## Subscriber

Message subscriber with receive methods.

#### Properties

| Property | Type | Description |
|----------|------|-------------|
| `topic` | `str` | Topic name |

#### Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `recv()` | Receive message (blocking) | `str` |
| `recv_with_timeout_ms(ms)` | Receive with timeout | `str | None` |
| `recv_json_with_timeout_ms(ms)` | Receive JSON with timeout | `Any | None` |
| `run(callback)` | Run callback loop for messages | `None` |
| `run_json(callback)` | Run JSON callback loop for messages | `None` |

#### Example

```python
from nbos import BusManager
import asyncio

async def main():
    async with BusManager() as bus:
        sub = await bus.create_subscriber("my/topic")
        msg = await sub.recv_with_timeout_ms(5000)
        print(f"Received: {msg}")

asyncio.run(main())
```

---

## Query / Queryable

Request-response pattern.

#### Query Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `query_text(payload)` | Send text query | `str` |
| `query_text_timeout_ms(payload, ms)` | Send text query with timeout | `str | None` |

#### Queryable Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `start()` | Start server | `None` |
| `run(handler)` | Run with handler function | `None` |
| `run_json(handler)` | Run with JSON handler function | `None` |

#### Example

```python
from nbos import BusManager
import asyncio

def uppercase_handler(text: str) -> str:
    return text.upper()

async def main():
    async with BusManager() as bus:
        # Server
        queryable = await bus.create_queryable("svc/upper", uppercase_handler)
        await queryable.start()
        
        # Client
        query = await bus.create_query("svc/upper")
        result = await query.query_text("hello")  # "HELLO"
        print(result)

asyncio.run(main())
```

---

## Caller / Callable

RPC pattern.

#### Caller Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `call_text(payload)` | Call remote service | `str` |

#### Callable Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `start()` | Start server | `None` |
| `run(handler)` | Run with handler function | `None` |
| `run_json(handler)` | Run with JSON handler function | `None` |
| `is_started()` | Check if server is running | `bool` |

#### Example

```python
from nbos import BusManager
import asyncio

def echo_handler(text: str) -> str:
    return f"echo:{text}"

async def main():
    async with BusManager() as bus:
        # Server
        callable_srv = await bus.create_callable("svc/echo", echo_handler)
        await callable_srv.start()
        
        # Client
        caller = await bus.create_caller("svc/echo")
        result = await caller.call_text("ping")  # "echo:ping"
        print(result)

asyncio.run(main())
```

---

## ConfigLoader

Configuration loader for loading settings from various sources.

#### Constructor

```python
ConfigLoader(strategy="deep_merge")
```

Parameters:
- `strategy` (str): Merge strategy ('deep_merge', 'replace')

#### Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `discover()` | Auto-discover config files | `ConfigLoader` |
| `add_file(path)` | Add config file | `ConfigLoader` |
| `add_directory(path)` | Add config directory | `ConfigLoader` |
| `add_inline(data)` | Add inline configuration | `ConfigLoader` |
| `reset()` | Reset configuration | `ConfigLoader` |
| `load_sync()` | Load configuration synchronously | `dict` |
| `reload_sync()` | Reload configuration synchronously | `dict` |

#### Example

```python
from nbos import ConfigLoader

loader = ConfigLoader()
loader.discover()
loader.add_file("app.toml")
loader.add_inline({"agent": {"model": "gpt-4.1"}})
cfg = loader.load_sync()
print(cfg)
```

---

## AgentConfig / Agent

Configuration and agent creation utilities.

### AgentConfig

Create agent configuration:

```python
from nbos import AgentConfig

cfg = AgentConfig(
    name="assistant",
    model="gpt-4.1",
    api_key="sk-...",
    base_url="https://api.openai.com/v1",
)
```

### Agent

Create agent from configuration:

```python
from nbos import Agent, AgentConfig

cfg = AgentConfig(
    name="assistant",
    model="gpt-4.1",
    api_key="sk-...",
    base_url="https://api.openai.com/v1",
)
agent = Agent(cfg)
text = await agent.react("Say hello in one sentence")
print(text)
```

#### AgentConfig Methods

| Method | Description |
|--------|-------------|
| `discover()` | Auto-discover config files |
| `add_file(path)` | Add config file |
| `add_directory(path)` | Add config directory |
| `add_inline(data)` | Add inline config |
| `reset()` | Reset config |
| `load_sync()` | Load configuration |
| `reload_sync()` | Reload configuration |

### Agent Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `from_config(config)` | Create agent from config | `Agent` |
| `register(tool)` | Register a tool | `Agent` |
| `register_many(*tools)` | Register multiple tools | `Agent` |
| `ask(question)` | Run agent with ReAct reasoning | `str` |
| `chat(message)` | Simple chat | `str` |
| `run_simple(message)` | Simple run | `str` |
| `react(task)` | Run with ReAct reasoning | `str` |
| `stream(task)` | Stream response tokens | `AsyncIterator[str]` |

---

## Best Practices

- Use `asyncio.run(...)` at process entry and `await` all async API calls.
- Keep `AgentConfig` immutable after creation for reproducibility.
- Prefer `ConfigLoader` + inline overrides for environment-specific setup.
- Reuse one `Bus` instance per process, not per call.
- Keep query/call handlers fast and side-effect-light; move heavy IO to async tasks.

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
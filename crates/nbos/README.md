# nbos

> Python bindings for BrainOS — AI agent framework with ReAct engine

High-performance Python bindings for [BrainOS](https://github.com/open1s/bos), a Rust-based AI agent framework implementing the ReAct (Reason + Act) paradigm. Built with [PyO3](https://pyo3.rs/) for native performance.

## Features

- **ReAct Agent** — Async agent with tool-calling capabilities, streaming responses, and automatic reasoning
- **Message Bus** — Publish/subscribe message bus for inter-agent communication
- **Lifecycle Hooks** — Intercept and modify agent behavior at key points
- **Plugin System** — Extend LLM requests/responses and tool execution
- **MCP Client** — Connect to [Model Context Protocol](https://modelcontextprotocol.io/) servers
- **Python Tools** — Register Python functions as agent tools
- **Skills** — Register agent capabilities from directory files

## Installation

### From PyPI (when published)

```bash
pip install nbos

poetry add nbos
```

### Development

```bash
# Build and install in development mode
maturin develop -m crates/nbos/Cargo.toml

# Or with specific Python environment
maturin develop -m crates/nbos/Cargo.toml --venv .venv
```

### Requirements

- Python >= 3.9
- Rust toolchain (for building from source)

## Quick Start

```python
import asyncio
from nbos import Agent, AgentConfig

async def main():
    # Create agent configuration
    config = AgentConfig(
        name="assistant",
        model="gpt-4",
        api_key="your-api-key",
        base_url="https://api.openai.com/v1",
        system_prompt="You are a helpful assistant.",
        temperature=0.7,
    )

    # Create the agent
    agent = Agent.from_config(config)

    # Define a Python tool
    def calculator(args: dict) -> str:
        """Evaluate a mathematical expression."""
        expr = args.get("expression", "")
        try:
            result = eval(expr)
            return str(result)
        except Exception as e:
            return f"Error: {e}"

    # Register the tool
    tool = PythonTool(
        name="calculator",
        description="Evaluate a mathematical expression",
        parameters='{"expression": {"type": "string"}}',
        schema='{"type": "object", "properties": {"expression": {"type": "string"}}}',
        callback=calculator,
    )
    await agent.add_tool(tool)

    # Run a task
    result = await agent.run_simple("What is 15 * 23?")
    print(result)

asyncio.run(main())
```

## API Reference

### AgentConfig

Configuration for creating an agent.

```python
from nbos import AgentConfig

config = AgentConfig(
    name="my-agent",           # Agent name
    model="gpt-4",             # LLM model identifier
    base_url="https://api.openai.com/v1",
    api_key="your-api-key",
    system_prompt="You are helpful.",
    temperature=0.7,           # Sampling temperature (0-2)
    max_tokens=4096,           # Max tokens in response
    timeout_secs=120,          # Request timeout
    # Context compaction (for long conversations)
    context_compaction_threshold_tokens=100000,
    context_compaction_trigger_ratio=0.8,
    context_compaction_keep_recent_messages=10,
    context_compaction_summary_max_tokens=2000,
)
```

### Agent

The core AI agent class.

#### `Agent.from_config(config)` — Create agent from config

```python
agent = Agent.from_config(config)
```

#### `Agent.create(config, bus)` — Create with a message bus

```python
from nbos import Bus, BusConfig

bus = await Bus.create(BusConfig())
agent = await Agent.create(config, bus)
```

#### `agent.run_simple(task)` — Run a simple task

Execute a task with automatic tool calling.

```python
result = await agent.run_simple("What is 100 * 100?")
```

#### `agent.react(task)` — Run with ReAct reasoning

Execute using explicit ReAct reasoning loop.

```python
result = await agent.react("Find files modified in the last hour")
```

#### `agent.stream(task)` — Stream responses

Returns an async iterator for streaming token support.

```python
async for token in agent.stream("Write a story"):
    if token.startswith('{"type":'):
        import json
        data = json.loads(token)
        if data["type"] == "text":
            print(data["text"], end="")
        elif data["type"] == "tool_call":
            print(f"\n[Tool: {data['name']}]")
    else:
        print(token, end="")
```

#### `agent.add_tool(tool)` — Register a Python tool

```python
from nbos import PythonTool

def weather(args: dict) -> str:
    return '{"temperature": 22, "condition": "sunny"}'

tool = PythonTool(
    name="weather",
    description="Get weather for a city",
    parameters='{"city": {"type": "string"}}',
    schema='{"type": "object", "properties": {"city": {"type": "string"}}}',
    callback=weather,
)
await agent.add_tool(tool)
```

#### MCP Integration

```python
# Add MCP server (process-based)
await agent.add_mcp_server(
    "filesystem",           # namespace
    "npx",                  # command
    ["-y", "@modelcontextprotocol/server-filesystem", "/path"]  # args
)

# Add MCP server (HTTP)
await agent.add_mcp_server_http("mcp-server", "https://mcp.example.com/sse")

# List MCP tools
tools = await agent.list_mcp_tools()
resources = await agent.list_mcp_resources("namespace")
```

#### Skills

```python
await agent.register_skills_from_dir("./skills")
```

#### Hooks

```python
# Register a hook
async def my_hook(ctx: HookContext) -> str:
    print(f"Before tool: {ctx.agent_id}")
    return "continue"

await agent.register_hook(HookEvent.BeforeToolCall, my_hook)
```

#### Message Management

```python
# Add message to conversation
await agent.add_message({"role": "user", "content": "Hello"})

# Get all messages
messages = agent.get_messages()

# Save/restore conversation
agent.save_message_log("./conversation.json")
agent.restore_message_log("./conversation.json")

# Session context
agent.set_session_context({"key": "value"})
context = agent.session_context()
agent.clear_session_context()

# Full session
agent.save_session("./session.json")
agent.restore_session("./session.json")

# Compact for long conversations
agent.compact_message_log()
```

### PythonTool

A Python function wrapped as an agent tool.

```python
from nbos import PythonTool

tool = PythonTool(
    name="tool_name",
    description="What the tool does",
    parameters='{"arg1": {"type": "string"}}',  # JSON Schema
    schema='{"type": "object", "properties": {...}}',
    callback=my_function,
)
```

### Bus (Message Bus)

Distributed message bus for inter-agent communication.

#### `Bus.create(config?)` — Create a bus

```python
from nbos import Bus, BusConfig

bus = await Bus.create(BusConfig(mode="peer"))
```

#### Publishing

```python
await bus.publish_text("topic", "message")
await bus.publish_json("topic", {"data": "value"})
```

#### Publisher

```python
pub = await Publisher.create(bus, "my-topic")
await pub.publish_text("hello")
await pub.publish_json({"event": "data"})
```

#### Subscriber

```python
sub = await Subscriber.create(bus, "my-topic")

# Blocking receive
msg = await sub.recv()
msg = await sub.recv_with_timeout_ms(5000)

# Or process JSON
data = await sub.recv_json_with_timeout_ms(5000)

# Run a handler
await sub.run(lambda err, msg: print(msg))
await sub.stop()
```

### Query / Queryable

Request/response pattern.

```python
from nbos import Query, Queryable

# Server side
def handler(text: str) -> str:
    return text.upper()

queryable = await Queryable.create(bus, "svc/upper", handler)
await queryable.start()

# Client side
query = await Query.create(bus, "svc/upper")
result = await query.query_text("hello")  # "HELLO"
result = await query.query_text_timeout_ms("hello", 5000)
```

### Caller / Callable

RPC pattern.

```python
from nbos import Caller, Callable

# Server side
def echo_handler(text: str) -> str:
    return f"echo:{text}"

callable_srv = await Callable.create(bus, "svc/echo", echo_handler)
await callable_srv.start()

# Client side
caller = await Caller.create(bus, "svc/echo")
result = await caller.call_text("ping")  # "echo:ping"
```

### HookEvent / HookContext

Lifecycle hook system.

```python
from nbos import HookEvent, HookContext

# Hook events
HookEvent.BeforeToolCall
HookEvent.AfterToolCall
HookEvent.BeforeLlmCall
HookEvent.AfterLlmCall
HookEvent.OnMessage
HookEvent.OnComplete
HookEvent.OnError

# Context data
context.agent_id   # Agent identifier
context.data       # Dict[str, str] with event data
```

#### Hook Example — Logging tool calls

```python
import asyncio
from nbos import Agent, AgentConfig, PythonTool, HookEvent, HookDecision, HookContext

async def main():
    config = AgentConfig(
        name="assistant",
        model="gpt-4",
        api_key="your-api-key",
        base_url="https://api.openai.com/v1",
    )
    agent = Agent.from_config(config)

    # Register a hook for before tool calls
    def before_tool_hook(event: HookEvent, ctx: HookContext) -> HookDecision:
        print(f"[HOOK] {event.value} - Agent: {ctx.agent_id}")
        for key, value in ctx.data.items():
            print(f"  {key}: {value}")
        # Return decision: "Continue", "Abort", or HookDecision("Error", "message")
        return HookDecision("Continue", None)

    # Register for multiple events
    agent.register_hook(HookEvent("BeforeToolCall"), before_tool_hook)
    agent.register_hook(HookEvent("AfterToolCall"), before_tool_hook)
    agent.register_hook(HookEvent("BeforeLlmCall"), before_tool_hook)
    agent.register_hook(HookEvent("AfterLlmCall"), before_tool_hook)

    # Define a simple tool
    def calculator(args: dict) -> str:
        return str(eval(args.get("expression", "0")))

    tool = PythonTool(
        name="calc",
        description="Calculate math",
        parameters='{"expression": {"type": "string"}}',
        schema='{"type": "object", "properties": {"expression": {"type": "string"}}}',
        callback=calculator,
    )
    await agent.add_tool(tool)

    # Run — hooks will fire during execution
    result = await agent.run_simple("What is 5 + 3?")
    print(f"Result: {result}")

asyncio.run(main())
```

#### Hook Example — Blocking dangerous operations

```python
def security_hook(event: HookEvent, ctx: HookContext) -> HookDecision:
    if event.value == "BeforeToolCall":
        tool_name = ctx.data.get("tool_name", "")
        # Block certain tools
        if tool_name in ["delete_file", "drop_database"]:
            return HookDecision("Error", f"Blocked: {tool_name} is not allowed")
    return HookDecision("Continue", None)

agent.register_hook(HookEvent("BeforeToolCall"), security_hook)
```

### AgentPlugin

Plugin system for intercepting LLM requests/responses and tool execution.

```python
from nbos import AgentPlugin, LlmRequestWrapper, LlmResponseWrapper, ToolCallWrapper, ToolResultWrapper

# Create a plugin with callbacks
plugin = AgentPlugin(
    name="my-plugin",
    on_llm_request=handle_request,      # Called before LLM request
    on_llm_response=handle_response,    # Called after LLM response
    on_tool_call=handle_tool_call,      # Called before tool execution
    on_tool_result=handle_tool_result,  # Called after tool execution
)

# Register the plugin
agent.register_plugin(plugin)
```

#### Plugin Example — Logging and modifying requests

```python
import asyncio
from nbos import (
    Agent, AgentConfig, PythonTool, AgentPlugin,
    LlmRequestWrapper, LlmResponseWrapper, ToolCallWrapper, ToolResultWrapper
)

async def log_llm_request(req: LlmRequestWrapper) -> LlmRequestWrapper:
    """Log and optionally modify LLM request."""
    print(f"[PLUGIN] LLM Request: model={req.model}, temp={req.temperature}")
    # Optionally modify the request
    req.temperature = 0.5  # Override temperature
    return req

async def log_llm_response(resp: LlmResponseWrapper) -> LlmResponseWrapper:
    """Log LLM response."""
    print(f"[PLUGIN] LLM Response: type={resp.response_type}")
    if resp.content:
        print(f"  Content: {resp.content[:100]}...")
    return resp

async def log_tool_call(tool_call: ToolCallWrapper) -> ToolCallWrapper:
    """Log tool call before execution."""
    print(f"[PLUGIN] Tool call: {tool_call.name}({tool_call.id})")
    return tool_call

async def log_tool_result(tool_result: ToolResultWrapper) -> ToolResultWrapper:
    """Log tool result after execution."""
    print(f"[PLUGIN] Tool result: success={tool_result.success}")
    return tool_result

# Create agent with plugins
config = AgentConfig(
    name="assistant",
    model="gpt-4",
    api_key="your-api-key",
    base_url="https://api.openai.com/v1",
)
agent = Agent.from_config(config)

# Create and register plugin
plugin = AgentPlugin(
    name="logging-plugin",
    on_llm_request=log_llm_request,
    on_llm_response=log_llm_response,
    on_tool_call=log_tool_call,
    on_tool_result=log_tool_result,
)
agent.register_plugin(plugin)

# Add a tool and run
def echo(args: dict) -> str:
    return args.get("message", "")

tool = PythonTool(
    name="echo",
    description="Echo a message",
    parameters='{"message": {"type": "string"}}',
    schema='{"type": "object", "properties": {"message": {"type": "string"}}}',
    callback=echo,
)
await agent.add_tool(tool)

result = await agent.run_simple("Say hello")
print(result)

asyncio.run(main())
```

#### Plugin Example — Custom tool filtering

```python
async def filter_tool_calls(tool_call: ToolCallWrapper) -> ToolCallWrapper:
    """Modify tool arguments before execution."""
    # Add additional context to tool arguments
    import json
    args = json.loads(tool_call.args)
    args["_from_plugin"] = "true"
    tool_call.args = json.dumps(args)
    return tool_call

plugin = AgentPlugin(
    name="tool-filter",
    on_tool_call=filter_tool_calls,
)
agent.register_plugin(plugin)
```

### McpClient

Standalone MCP client.

```python
from nbos import McpClient

# From command
client = await McpClient.spawn("npx", ["-y", "server-filesystem", "/tmp"])
await client.initialize()

# Or HTTP
client = McpClient.connect_http("http://127.0.0.1:8000/mcp")
await client.initialize()

# Or HTTPS
client = McpClient.connect_http("https://mcp.example.com/mcp")
await client.initialize()

# Use
tools = await client.list_tools()
result = await client.call_tool("tool-name", '{"arg": "value"}')
prompts = await client.list_prompts()
resources = await client.list_resources()
resource = await client.read_resource("resource-uri")
```

### ConfigLoader

Load configuration from files.

```python
from nbos import ConfigLoader

loader = ConfigLoader(strategy="deep_merge")
loader.add_file("app.toml")
loader.add_inline({"agent": {"model": "gpt-4"}})
config = loader.load_sync()  # Returns JSON string
```

### LlmMessage

Message types for conversation history.

```python
from nbos import LlmMessage

msg = LlmMessage.system("System prompt")
msg = LlmMessage.user("User message")
msg = LlmMessage.assistant("Assistant response")
msg = LlmMessage.assistant_tool_call("tool_name", "tool_call_id", {"arg": "value"})
msg = LlmMessage.tool_result("tool_call_id", "result")

# Convert to dict
msg.to_py()  # Returns Python dict

# Create from dict
msg = LlmMessage.from_py({"role": "user", "content": "Hello"})
```

### Logging

```python
from nbos import init_tracing, log_test_message

init_tracing()
log_test_message("Debug message")
```

## Examples

See the [examples](./examples/) directory:

| Example | Description |
|---------|-------------|
| `01_quickstart.py` | Basic agent setup |
| `02_multi_tool.py` | Multiple tools |
| `03_conversation.py` | Conversation history |
| `03_three_modes.py` | Different execution modes |

Run an example:
```bash
python examples/01_quickstart.py
```

## Best Practices

- Use `asyncio.run(...)` at process entry and `await` all async API calls
- Keep `AgentConfig` immutable after creation for reproducibility
- Prefer `ConfigLoader` + inline overrides for environment-specific setup
- Reuse one `Bus` instance per process, not per call
- Keep query/call handlers fast and side-effect-light; move heavy I/O to async tasks

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                         Python                               │
├─────────────────────────────────────────────────────────────┤
│  nbos (PyO3 bindings)                                  │
│  ┌─────────┐ ┌──────┐ ┌───────┐ ┌───────┐ ┌─────────────┐  │
│  │  Agent  │ │ Bus  │ │ Hooks │ │Plugins│ │   MCP Client│  │
│  └────┬────┘ └──┬───┘ └───┬───┘ └───┬───┘ └──────┬──────┘  │
└───────┼────────┼─────────┼─────────┼────────────┼──────────┘
        │        │         │         │            │
        ▼        ▼         ▼         ▼            ▼
┌─────────────────────────────────────────────────────────────┐
│                    BrainOS (Rust Core)                       │
│  agent/ │ bus/ │ hooks/ │ mcp/ │ plugin/ │ config/          │
└─────────────────────────────────────────────────────────────┘
```

## Related

- [@open1s/jsbos](../jsbos/) — JavaScript/Node.js bindings
- [BrainOS](https://github.com/open1s/bos) — Core Rust framework
- Maturin publis: export MATURIN_PYPI_TOKEN=...
# BrainOS JavaScript API User Guide

This guide provides a unified, consistent API for using BrainOS in JavaScript/Node.js. The API mirrors the Python API for a seamless cross-language experience.

## Table of Contents

1. [Installation](#installation)
2. [Quick Start](#quick-start)
3. [Core Concepts](#core-concepts)
4. [Agent API](#agent-api)
5. [Tool Registration](#tool-registration)
6. [Hooks](#hooks)
7. [Bus Communication](#bus-communication)
8. [Query/Queryable](#queryqueryable)
9. [Caller/Callable](#callercallable)
10. [Configuration](#configuration)
11. [MCP Client](#mcp-client-model-context-protocol)
12. [API Reference](#api-reference)

---

## Installation

```bash
npm install @open1s/jsbos
# Or use the alias:
npm install brainos
```

Or build from source:

```bash
cd crates/jsbos
npm install
npm run build
```

---

## Quick Start

```javascript
const { BrainOS, tool } = require('@open1s/jsbos/brainos');

async function main() {
  const brain = new BrainOS();
  await brain.start();
  
  const agent = brain.agent('assistant');
  const result = await agent.ask('What is 42 + 58?');
  console.log(result);
  
  await brain.stop();
}

main().catch(console.error);
```

---

## Core Concepts

### BrainOS (Main Entry Point)

The `BrainOS` class manages the lifecycle:

```javascript
const brain = new BrainOS();
await brain.start();
// brain is ready to use
const agent = brain.agent('my-agent');
await brain.stop();
```

### Agent

An `Agent` represents an LLM-powered agent that can use tools:

```javascript
const agent = brain.agent('assistant');
const agent = brain.agent('coder', { systemPrompt: 'You are a coding assistant.' });
```

### Tool

Tools are functions that the LLM can call. Use the `tool()` decorator:

```javascript
// Note: Due to JavaScript's nature, tools work slightly differently
// See Tool Registration section for details
```

---

## Agent API

### Creating an Agent

```javascript
// Basic agent
const agent = brain.agent('assistant');

// Agent with custom config
const agent = brain.agent('coder', {
  systemPrompt: 'You are a helpful coding assistant.',
  model: 'nvidia/meta/llama-3.1-8b-instruct',
  baseUrl: 'https://integrate.api.nvidia.com/v1',
  temperature: 0.5,
  timeoutSecs: 180,
});
```

### Fluent Configuration

Use chainable methods to configure the agent:

```javascript
const agent = brain.agent('assistant')
  .withModel('nvidia/meta/llama-3.1-8b-instruct')
  .withTemperature(0.3)
  .withPrompt('You are a math tutor.')
  .withTimeout(300);
```

### Running the Agent

```javascript
// Simple Q&A (no tool use)
const result = await agent.ask('What is Python?');

// Run with tool use enabled
const result = await agent.react('Calculate 2 + 2');

// Simple conversation
const result = await agent.chat('Hello!');
const result = await agent.runSimple('Hello!');
```

### Registering Tools

```javascript
// First create a ToolDef, then register
const toolDef = new ToolDef(
  'add',
  'Add two numbers',
  (args) => args.a + args.b,
  { a: { type: 'integer' }, b: { type: 'integer' } },
  { type: 'object', properties: { a: { type: 'integer' }, b: { type: 'integer' } }}
);

agent.register(toolDef);
agent.registerMany(tool1, tool2);
```

---

## Hooks

Hooks allow you to intercept and react to events during agent execution. Use `onHook()` to register callback functions.

### Using Hooks with BrainOS Agent

```javascript
const { Agent, HookEvent } = require('@open1s/jsbos');

const brain = new BrainOS({ apiKey: 'sk-...' });
await brain.start();

// Register hooks
brain.agent('assistant')
  .onHook(HookEvent.BeforeToolCall, (ctx) => {
    console.log('[BeforeToolCall]', ctx.data.tool_name);
    return 'continue';  // proceed normally
  })
  .onHook(HookEvent.AfterToolCall, (ctx) => {
    console.log('[AfterToolCall]', ctx.data.tool_name);
    return 'continue';
  })
  .onHook(HookEvent.BeforeLlmCall, (ctx) => {
    console.log('[BeforeLlmCall] Starting LLM call');
    return 'continue';
  })
  .onHook(HookEvent.AfterLlmCall, (ctx) => {
    console.log('[AfterLlmCall] LLM call completed');
    return 'continue';
  })
  .onHook(HookEvent.OnError, (ctx) => {
    console.log('[OnError]', ctx.data.error);
    return 'continue';
  });
```

### Using Hooks with Raw Agent

```javascript
const { Agent, HookEvent } = require('./index.js');

const agent = await Agent.create({
  name: 'assistant',
  model: 'gpt-4',
  apiKey: 'sk-...',
  baseUrl: 'https://api.openai.com/v1',
  systemPrompt: 'You are helpful.',
  temperature: 0.7,
  timeoutSecs: 120,
});

agent.registerHook(HookEvent.BeforeToolCall, (ctx) => {
  console.log('[BeforeToolCall]', ctx.data.tool_name);
  return 'continue';
});

agent.registerHook(HookEvent.AfterToolCall, (ctx) => {
  console.log('[AfterToolCall]', ctx.data.tool_name);
  return 'continue';
});
```

### Hook Events

| Event | Description |
|-------|-------------|
| `BeforeToolCall` | Fired before a tool is called |
| `AfterToolCall` | Fired after a tool completes |
| `BeforeLlmCall` | Fired before LLM API call |
| `AfterLlmCall` | Fired after LLM API call |
| `OnMessage` | Fired for each message |
| `OnComplete` | Fired when agent completes |
| `OnError` | Fired when an error occurs |

### Hook Decisions

Return a string to control execution:

| Decision | Description |
|---------|-------------|
| `'continue'` | Proceed normally (default) |
| `'abort'` | Abort current operation |
| `'error:message'` | Return an error |

### Hook Context

The callback receives a `HookContextData` object:

```javascript
{
  agent_id: 'assistant',     // agent name
  data: {                   // event-specific data
    tool_name: 'add',      // for BeforeToolCall/AfterToolCall
    error: 'failed',        // for OnError
    // ...
  }
}
```

---

## Tool Registration

## Tool Registration

### Using the `tool()` Decorator

Due to JavaScript's limitations with decorators, the recommended approach is using `ToolDef`:

```javascript
const { ToolDef } = require('@open1s/jsbos/brainos');

// Simple function tool
function add(args) {
  return args.a + args.b;
}

const toolDef = new ToolDef(
  'add',                           // name
  'Add two numbers together',       // description
  add,                              // callback function
  { a: { type: 'integer' }, b: { type: 'integer' } },  // parameters
  {                                 // schema
    type: 'object',
    properties: {
      a: { type: 'integer', description: 'First number' },
      b: { type: 'integer', description: 'Second number' }
    },
    required: ['a', 'b']
  }
);

// Register with agent
agent.register(toolDef);
```

### Tool with JSON Schema

```javascript
const weatherTool = new ToolDef(
  'get_weather',
  'Get weather information for a city',
  (args) => {
    // Simulated weather data
    return JSON.stringify({
      city: args.city,
      temperature: 22,
      unit: 'celsius',
      condition: 'sunny'
    });
  },
  { city: { type: 'string' } },  // parameters
  {                               // schema
    type: 'object',
    properties: {
      city: {
        type: 'string',
        description: 'City name, e.g., "Beijing", "San Francisco"'
      }
    },
    required: ['city']
  }
);

agent.register(weatherTool);
```

### Using the Decorator (Experimental)

```javascript
const { tool } = require('@open1s/jsbos/brainos');

class MyTools {
  @tool('Add two numbers')
  add(args) {
    return args.a + args.b;
  }
  
  @tool('Multiply two numbers', { name: 'multiply' })
  multiply(args) {
    return args.a * args.b;
  }
}

// Then extract toolDefs and register
const instance = new MyTools();
// Access: instance.add.toolDef
```

---

## Bus Communication

The Bus provides pub/sub messaging between components.

### Using BusManager

```javascript
const { BusManager } = require('@open1s/jsbos/brainos');

const bus = await BusManager.create();
await bus.start();

// Publish
await bus.publishText('my/topic', 'hello');
await bus.publishJson('my/topic', { data: 123 });

// Create publisher
const pub = await bus.createPublisher('output/topic');
await pub.publishText('message');

// Create subscriber
const sub = await bus.createSubscriber('input/topic');
const msg = await sub.recv();

await bus.stop();
```

### Subscriber Patterns

```javascript
const sub = await bus.createSubscriber('my/topic');

// One-shot receive
let msg = await sub.recv();
msg = await sub.recvWithTimeoutMs(5000);

// Get JSON
const data = await sub.recvJsonWithTimeoutMs(5000);

// Callback loop
await sub.run((msg) => console.log(`Received: ${msg}`));

// Async iteration
for await (const msg of sub) {
  console.log(msg);
}
```

---

## Query/Queryable

Request-response pattern with timeout support. Uses `BusManager` factory methods.

### Server Side (Queryable)

```javascript
const { BusManager } = require('@open1s/jsbos/brainos');

const bus = await BusManager.create();
await bus.start();

// Create and start queryable server
const q = await bus.createQueryable('svc/upper', (text) => text.toUpperCase());
await q.start();
```

### Client Side (Query)

```javascript
const query = await bus.createQuery('svc/upper');
const result = await query.queryText('hello');  // "HELLO"
const result = await query.queryTextTimeoutMs('hello', 5000);  // with timeout
```

### QueryClient API

**Properties:**
| Property | Type | Description |
|-----------|------|-------------|
| `topic` | `string` | Topic name |

**Methods:**
| Method | Description |
|--------|-------------|
| `queryText(payload)` | Send text query |
| `queryTextTimeoutMs(payload, ms)` | Send with timeout |

### QueryableServer API

**Methods:**
| Method | Description |
|--------|-------------|
| `setHandler(handler)` | Set handler function |
| `start()` | Start server |
| `run(handler)` | Run with handler |
| `runJson(handler)` | Run JSON handler |

---

## Caller/Callable

RPC-style request-response pattern. Uses `BusManager` factory methods.

### Server Side (Callable)

```javascript
const { BusManager } = require('@open1s/jsbos/brainos');

const bus = await BusManager.create();
await bus.start();

// Create and start callable server
const srv = await bus.createCallable('svc/echo', (text) => `echo: ${text}`);
await srv.start();
```

### Client Side (Caller)

```javascript
const caller = await bus.createCaller('svc/echo');
const result = await caller.callText('ping');  // "echo: ping"
```

### CallerClient API

**Methods:**
| Method | Description |
|--------|-------------|
| `callText(payload)` | Call remote service |

### CallableServer API

**Properties:**
| Property | Type | Description |
|-----------|------|-------------|
| `isStarted` | `boolean` | Whether server is running |

**Methods:**
| Method | Description |
|--------|-------------|
| `setHandler(handler)` | Set handler function |
| `start()` | Start server |
| `run(handler)` | Run with handler |
| `runJson(handler)` | Run JSON handler |

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

### Using ConfigLoader Class

```javascript
const { ConfigLoader } = require('@open1s/jsbos/brainos');

const loader = new ConfigLoader();
loader.discover();
loader.addFile('/path/to/config.toml');
loader.addDirectory('/path/to/conf');
loader.addInline({ key: 'value' });

const config = JSON.parse(loader.loadSync());
```

### Environment Variables

- `OPENAI_API_KEY` - API key for OpenAI (or `BOS_API_KEY`)
- `LLM_BASE_URL` - Base URL for LLM API (or `BOS_BASE_URL`)
- `LLM_MODEL` - Model name (or `BOS_MODEL`)

---

## MCP Client (Model Context Protocol)

BrainOS supports MCP (Model Context Protocol) for connecting to external tools and services.

### Using MCP Client

```javascript
const { McpClient } = require('@open1s/jsbos/brainos');

async function main() {
  // Spawn an MCP server process
  const client = await McpClient.spawn('npx', ['-y', '@modelcontextprotocol/server-filesystem', '/tmp']);
  await client.initialize();

  // List available tools
  const tools = await client.listTools();
  console.log('Available tools:', tools);

  // Call a tool
  const result = await client.callTool('tool_name', JSON.stringify({ arg: 'value' }));
  console.log(result);

  // List prompts
  const prompts = await client.listPrompts();
  console.log(prompts);

  // List resources
  const resources = await client.listResources();
  console.log(resources);

  // Read a resource
  const resource = await client.readResource('resource://path/to/resource');
  console.log(resource);
}

main().catch(console.error);
```

### Connect via HTTP

```javascript
const { McpClient } = require('@open1s/jsbos/brainos');

const client = McpClient.connectHttp('http://localhost:3000');
await client.initialize();
const tools = await client.listTools();
```

### MCP Client API

| Method | Description |
|--------|-------------|
| `McpClient.spawn(command, args)` | Spawn an MCP server process |
| `McpClient.connect_http(url)` | Connect via HTTP |
| `initialize()` | Initialize MCP connection |
| `list_tools()` | List available tools |
| `call_tool(name, args_json)` | Call a tool with JSON args |
| `list_prompts()` | List available prompts |
| `list_resources()` | List available resources |
| `read_resource(uri)` | Read a resource by URI |

---

## API Reference

For the complete API reference, see [JavaScript API Reference](./api-reference/jsbos-api.md).

### `BrainOS`

Main entry point for BrainOS.

**Constructor:**
```javascript
new BrainOS(options = {})
// options: { apiKey, baseUrl, model }
```

**Methods:**
| Method | Description |
|--------|-------------|
| `start()` | Initialize BrainOS |
| `stop()` | Shutdown BrainOS |
| `agent(name, options)` | Create a new agent |

**Properties:**
| Property | Type | Description |
|-----------|------|-------------|
| `bus` | `Bus` | The underlying Bus |

### `Agent`

LLM-powered agent with tool support.

**Constructor:**
```javascript
new Agent(bus, options = {})
// options: { name, model, baseUrl, apiKey, systemPrompt, temperature, timeoutSecs }
// Defaults: model: 'nvidia/meta/llama-3.1-8b-instruct', baseUrl: 'https://integrate.api.nvidia.com/v1'
```

**Methods:**
| Method | Description | Returns |
|--------|-------------|---------|
| `withModel(model)` | Set model | `Agent` |
| `withPrompt(prompt)` | Set system prompt | `Agent` |
| `withTemperature(temp)` | Set temperature | `Agent` |
| `withTimeout(secs)` | Set timeout | `Agent` |
| `register(toolDef)` | Register a tool | `Agent` |
| `registerMany(...toolDefs)` | Register multiple tools | `Agent` |
| `start()` | Initialize agent | `Agent` |
| `ask(question)` | Run agent | `Promise<string>` |
| `chat(message)` | Simple chat | `Promise<string>` |
| `runSimple(message)` | Simple run | `Promise<string>` |
| `react(task)` | Run with ReAct | `Promise<string>` |

### Resilience Configuration

The Agent supports configuring circuit breaker and rate limiter for resilience:

```javascript
const { Agent } = require('@open1s/jsbos/brainos');

const agent = await Agent.create({
  name: "assistant",
  model: "gpt-4",
  apiKey: "sk-...",
  // Circuit Breaker - prevents cascading failures
  circuitBreakerMaxFailures: 5,         // failures before opening circuit
  circuitBreakerCooldownSecs: 30,       // seconds before attempting recovery

  // Rate Limiter - prevents 429 errors
  rateLimitCapacity: 40,               // max requests per window
  rateLimitWindowSecs: 60,             // window duration in seconds
  rateLimitMaxRetries: 3,            // retry attempts on rate limit
  rateLimitRetryBackoffSecs: 1,         // backoff between retries
  rateLimitAutoWait: true,             // auto-wait when rate limited
});
```

**Circuit Breaker Options:**
| Option | Default | Description |
|--------|---------|-------------|
| `circuitBreakerMaxFailures` | 5 | Failures before opening circuit |
| `circuitBreakerCooldownSecs` | 30 | Seconds before half-open state |

**Rate Limiter Options:**
| Option | Default | Description |
|--------|---------|-------------|
| `rateLimitCapacity` | 40 | Max requests per window |
| `rateLimitWindowSecs` | 60 | Window duration in seconds |
| `rateLimitMaxRetries` | 3 | Retry attempts on 429 errors |
| `rateLimitRetryBackoffSecs` | 1 | Initial backoff duration |
| `rateLimitAutoWait` | true | Auto-wait when rate limited |

**Properties:**
| Property | Type | Description |
|-----------|------|-------------|
| `tools` | `string[]` | Registered tool names |
| `config` | `object` | Agent configuration |

### `ToolDef`

Tool definition class.

**Constructor:**
```javascript
new ToolDef(name, description, callback, parameters, schema)
```

### `BusManager`

Async context manager for Bus lifecycle.

**Constructor:**
```javascript
new BusManager(options = {})
// options: { mode, connect, listen, peer }
```

**Static Methods:**
| Method | Description |
|--------|-------------|
| `BusManager.create(options)` | Create and start BusManager |

**Methods:**
| Method | Description |
|--------|-------------|
| `start()` | Initialize bus |
| `stop()` | Shutdown bus |
| `publishText(topic, payload)` | Publish text message |
| `publishJson(topic, data)` | Publish JSON message |
| `createPublisher(topic)` | Create a publisher |
| `createSubscriber(topic)` | Create a subscriber |
| `createQuery(topic)` | Create a query client |
| `createQueryable(topic, handler)` | Create a queryable server |
| `createCaller(name)` | Create a caller client |
| `createCallable(uri, handler)` | Create a callable server |

### `PublisherWrapper`

Message publisher for a specific topic.

**Properties:**
| Property | Type | Description |
|-----------|------|-------------|
| `topic` | `string` | Topic name |

**Methods:**
| Method | Description |
|--------|-------------|
| `publishText(payload)` | Publish text |
| `publishJson(data)` | Publish JSON |

### `SubscriberWrapper`

Message subscriber with receive methods.

**Properties:**
| Property | Type | Description |
|-----------|------|-------------|
| `topic` | `string` | Topic name |

**Methods:**
| Method | Description |
|--------|-------------|
| `recv()` | Receive message (blocking) |
| `recvWithTimeoutMs(ms)` | Receive with timeout |
| `recvJsonWithTimeoutMs(ms)` | Receive JSON with timeout |
| `run(callback)` | Run callback loop |
| `runJson(callback)` | Run JSON callback loop |

### `QueryClient` / `QueryableServer`

Request-response pattern.

**QueryClient Methods:**
| Method | Description |
|--------|-------------|
| `queryText(payload)` | Send query |
| `queryTextTimeoutMs(payload, ms)` | Send with timeout |

**QueryableServer Methods:**
| Method | Description |
|--------|-------------|
| `setHandler(handler)` | Set handler |
| `start()` | Start server |
| `run(handler)` | Run with handler |
| `runJson(handler)` | Run JSON handler |

### `CallerClient` / `CallableServer`

RPC pattern.

**CallerClient Methods:**
| Method | Description |
|--------|-------------|
| `callText(payload)` | Call remote service |

**CallableServer Methods:**
| Method | Description |
|--------|-------------|
| `setHandler(handler)` | Set handler |
| `start()` | Start server |
| `run(handler)` | Run with handler |
| `runJson(handler)` | Run JSON handler |
| `isStarted` | Check if running |

### `ConfigLoader`

Configuration loader.

**Constructor:**
```javascript
new ConfigLoader()
```

**Methods:**
| Method | Description |
|--------|-------------|
| `discover()` | Auto-discover config files |
| `addFile(path)` | Add config file |
| `addDirectory(path)` | Add config directory |
| `addInline(data)` | Add inline config |
| `reset()` | Reset config |
| `loadSync()` | Load configuration |
| `reloadSync()` | Reload configuration |

### `McpClient`

MCP (Model Context Protocol) client for connecting to external tools and services.

**Static Factory Methods:**
| Method | Description |
|--------|-------------|
| `McpClient.spawn(command, args)` | Spawn an MCP server process |
| `McpClient.connect_http(url)` | Connect via HTTP URL |

**Methods:**
| Method | Description |
|--------|-------------|
| `initialize()` | Initialize MCP connection |
| `list_tools()` | List available tools |
| `call_tool(name, args_json)` | Call a tool with JSON string args |
| `list_prompts()` | List available prompts |
| `list_resources()` | List available resources |
| `read_resource(uri)` | Read a resource by URI |

---

## Examples

### Complete Example with Tools

```javascript
const { BrainOS, ToolDef } = require('@open1s/jsbos/brainos');

function add(args) {
  return args.a + args.b;
}

function multiply(args) {
  return args.a * args.b;
}

function getTime(args) {
  return JSON.stringify({ utc: new Date().toISOString() });
}

async function main() {
  const brain = new BrainOS();
  await brain.start();

  const agent = brain.agent('assistant')
    .register(new ToolDef(
      'add', 'Add two numbers', add,
      { a: { type: 'integer' }, b: { type: 'integer' } },
      { type: 'object', properties: { a: { type: 'integer' }, b: { type: 'integer' } }, required: ['a', 'b'] }
    ))
    .register(new ToolDef(
      'multiply', 'Multiply two numbers', multiply,
      { a: { type: 'integer' }, b: { type: 'integer' } },
      { type: 'object', properties: { a: { type: 'integer' }, b: { type: 'integer' } }, required: ['a', 'b'] }
    ))
    .register(new ToolDef(
      'get_time', 'Get current time', getTime,
      {}, { type: 'object', properties: {} }
    ));

  // Ask with tool use
  const result = await agent.react('What is 5 + 3? What is 4 * 7?');
  console.log(result);

  await brain.stop();
}

main().catch(console.error);
```

### Pub/Sub Example

```javascript
const { BusManager } = require('@open1s/jsbos/brainos');

async function publisher() {
  const bus = await BusManager.create();
  await bus.start();
  
  await bus.publishText('events/start', 'Hello subscribers!');
  await bus.stop();
}

async function subscriber() {
  const bus = await BusManager.create();
  await bus.start();
  
  const sub = await bus.createSubscriber('events/start');
  const msg = await sub.recvWithTimeoutMs(5000);
  console.log(`Received: ${msg}`);
  
  await bus.stop();
}

// Run both in separate processes
```

### Query/Response Example

```javascript
const { BusManager } = require('@open1s/jsbos/brainos');

function uppercase(text) {
  return text.toUpperCase();
}

async function main() {
  const bus = await BusManager.create();
  await bus.start();

  // Server
  const q = await bus.createQueryable('svc/uppercase', uppercase);
  await q.start();

  // Client
  const query = await bus.createQuery('svc/uppercase');
  const result = await query.queryText('hello world');
  console.log(result);  // "HELLO WORLD"

  await bus.stop();
}

main().catch(console.error);
```

---

## Hooks, Plugins, and Sessions

Hooks, plugins, and session management are available through the low-level `jsbos` bindings.

### Hooks (jsbos)

Hooks allow you to intercept and react to events during agent execution.

#### Using Hooks

```javascript
const { Agent, HookEvent } = require('@open1s/jsbos');

async function main() {
  const brain = new BrainOS();
  await brain.start();
  
  const agent = brain.agent('assistant');
  
  // Register hooks
  agent.hooks().register(HookEvent.BeforeToolCall, (ctx) => {
    return {
      toolName: ctx.data.tool_name,
      decision: 'continue'  // or 'abort' or { decision: 'error', message: 'error message' }
    };
  });
  
  agent.hooks().register(HookEvent.AfterToolCall, (ctx) => {
    return {
      toolName: ctx.data.tool_name,
      toolResult: ctx.data.tool_result,
      decision: 'continue'
    };
  });
  
  agent.hooks().register(HookEvent.BeforeLlmCall, (ctx) => {
    return {
      prompt: ctx.data.prompt,
      decision: 'continue'
    };
  });
  
  agent.hooks().register(HookEvent.AfterLlmCall, (ctx) => {
    return {
      response: ctx.data.response,
      decision: 'continue'
    };
  });
  
  agent.hooks().register(HookEvent.OnError, (ctx) => {
    return {
      error: ctx.data.error,
      decision: 'continue'
    };
  });
  
  const result = await agent.ask('Hello');
  console.log(result);
  
  await brain.stop();
}

main().catch(console.error);
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

Return an object to control execution:

| Decision | Description |
|----------|-------------|
| `{ decision: 'continue' }` | Proceed normally (default) |
| `{ decision: 'abort' }` | Abort current operation |
| `{ decision: 'error', message: 'error message' }` | Return an error |

### Plugins

Plugins allow you to preprocess and postprocess LLM requests and responses.

#### Using Plugins

```javascript
const { BrainOS } = require('@open1s/jsbos/brainos');

class MyPlugin {
  async processLlmRequest(wrapper) {
    // Modify request before sending to LLM
    // Example: add system prompt prefix
    const request = wrapper.intoRequest();
    // Modify request here
    return wrapper.constructor(request);
  }
  
  async processLlmResponse(wrapper) {
    // Modify response after receiving from LLM
    const response = wrapper.intoResponse();
    // Modify response here
    return wrapper.constructor(response);
  }
}

async function main() {
  const brain = new BrainOS();
  await brain.start();
  
  const agent = brain.agent('assistant');
  
  // Register plugin
  agent.plugins().registerBlocking(new MyPlugin());
  
  const result = await agent.ask('Hello');
  console.log(result);
  
  await brain.stop();
}

main().catch(console.error);
```

### Session Management

BrainOS provides session management for persisting agent state across restarts.

#### Session Operations

```javascript
const { BrainOS } = require('@open1s/jsbos/brainos');

async function main() {
  const brain = new BrainOS();
  await brain.start();
  
  const agent = brain.agent('assistant');
  
  // Save session
  agent.saveMessageLog('/tmp/session.json');
  
  // Later, restore session
  // agent.restoreMessageLog('/tmp/session.json');
  
  const result = await agent.ask('Hello');
  console.log(result);
  
  await brain.stop();
}

main().catch(console.error);
```

#### Session Info Methods

| Method | Description |
|--------|-------------|
| `addMessage(message)` | Add message to conversation log |
| `getMessages()` | Get conversation messages |
| `saveMessageLog(path)` | Save message log to file |
| `restoreMessageLog(path)` | Restore message log from file |

---

## Error Handling

---

## Differences from Python API

While the JavaScript API mirrors the Python API for consistency, there are some differences:

| Feature | Python | JavaScript |
|---------|--------|------------|
| Tool decorator | `@tool("desc")` | `ToolDef` class |
| Type hints | Native | JSDoc/TypeScript |
| Context manager | `async with` | `await brain.start()/stop()` |
| Async iteration | `async for` | `for await` |
| Class naming | `Subscriber` | `SubscriberWrapper` |

---

## TypeScript Support

If using TypeScript, import types from the main package:

```typescript
import { BrainOS, Agent, ToolDef } from 'brainos';

const brain = new BrainOS();
await brain.start();

const agent: Agent = brain.agent('assistant');
```

The type definitions are available in `index.d.ts`.

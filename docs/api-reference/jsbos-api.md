# BrainOS JavaScript API Reference

This document provides the complete API reference for the BrainOS JavaScript/Node.js bindings (`brainos` package).

## Main Entry Point

### BrainOS

Main entry point for BrainOS functionality.

#### Constructor

```javascript
new BrainOS(options = {})
```

Options:
- `apiKey` (string, optional): API key for LLM provider
- `baseUrl` (string, optional): Base URL for LLM API
- `model` (string, optional): Model name to use

#### Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `start()` | Initialize BrainOS | `Promise<void>` |
| `stop()` | Shutdown BrainOS | `Promise<void>` |
| `agent(name, options)` | Create a new agent | `Agent` |

#### Properties

| Property | Type | Description |
|----------|------|-------------|
| `bus` | `Bus` | The underlying Bus instance |

#### Example

```javascript
import { BrainOS } from 'brainos';

const brain = new BrainOS({
  apiKey: 'sk-...',
  baseUrl: 'https://api.openai.com/v1',
  model: 'gpt-4'
});

await brain.start();
// ... use brain ...
await brain.stop();
```

---

## Agent

LLM-powered agent with tool support.

### Constructor

```javascript
new Agent(bus, options = {})
```

Options:
- `name` (string): Agent name
- `model` (string): Model name (default: 'nvidia/meta/llama-3.1-8b-instruct')
- `baseUrl` (string): Base URL for LLM API
- `apiKey` (string): API key for LLM
- `systemPrompt` (string): System prompt for the agent
- `temperature` (number): Temperature for sampling (default: 0.7)
- `timeoutSecs` (number): Timeout in seconds (default: 120)

### Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `withModel(model)` | Set model | `Agent` |
| `withPrompt(prompt)` | Set system prompt | `Agent` |
| `withTemperature(temp)` | Set temperature | `Agent` |
| `withTimeout(secs)` | Set timeout | `Agent` |
| `withTools(...toolDefs)` | Register tools to be wired at start | `Agent` |
| `withBashTool(name, workspaceRoot)` | Add a Bash tool | `Agent` |
| `onHook(event, callback)` | Register a lifecycle hook | `Agent` |
| `register(toolDef)` | Register a tool | `Agent` |
| `registerMany(...toolDefs)` | Register multiple tools | `Agent` |
| `start()` | Initialize agent | `Promise<Agent>` |
| `ask(question)` | Run agent with ReAct reasoning | `Promise<string>` |
| `chat(message)` | Simple chat | `Promise<string>` |
| `runSimple(message)` | Simple run (no tool use) | `Promise<string>` |
| `react(task)` | Run with ReAct reasoning | `Promise<string>` |
| `stream(task, onToken)` | Stream response tokens | `Promise<string>` |
| `streamCollect(task)` | Collect all streaming tokens | `Promise<string>` |
| `tokenUsage()` | Get current token usage | `TokenUsage` |
| `tokenBudgetReport()` | Get token budget report | `TokenBudgetReport` |
| `getPerfMetrics()` | Get performance metrics | `object` |
| `resetPerfMetrics()` | Reset performance metrics | `void` |

#### Properties

| Property | Type | Description |
|----------|------|-------------|
| `tools` | `string[]` | Registered tool names |
| `config` | `object` | Agent configuration |

#### Token Usage

The Agent provides methods to monitor token consumption:

| Method | Description | Returns |
|--------|-------------|---------|
| `tokenUsage()` | Get current token usage statistics | `TokenUsage` |
| `tokenBudgetReport()` | Get detailed token budget report with status | `TokenBudgetReport` |

#### Classes

| Class | Description |
|-------|-------------|
| `TokenUsage` | Token usage statistics (prompt, completion, total) |
| `TokenBudgetReport` | Budget report with status and usage percentage |
| `BudgetStatus` | Enum: Normal, Warning, Exceeded, Critical |

#### Resilience Configuration

The Agent supports configuring circuit breaker and rate limiter for resilience:

```javascript
const agent = await Agent.create({
  name: "assistant",
  model: "gpt-4",
  apiKey: "sk-...",
  // Circuit Breaker - prevents cascading failures
  circuitBreakerMaxFailures: 5,      // failures before opening circuit
  circuitBreakerCooldownSecs: 30,      // seconds before attempting recovery
  // Rate Limiter - prevents 429 errors
  rateLimitCapacity: 40,            // max requests per window
  rateLimitWindowSecs: 60,          // window duration in seconds
  rateLimitMaxRetries: 3,             // retry attempts on rate limit
  rateLimitRetryBackoffSecs: 1,        // backoff between retries
  rateLimitAutoWait: true,            // auto-wait when rate limited
});
```

#### Example

```javascript
import { BrainOS, ToolDef } from 'brainos';

const brain = new BrainOS();
await brain.start();

const agent = brain.agent('assistant')
  .register(new ToolDef(
    'add', 'Add two numbers', 
    (args) => args.a + args.b,
    { a: { type: 'integer' }, b: { type: 'integer' } },
    { type: 'object', properties: { a: { type: 'integer' }, b: { type: 'integer' } }, required: ['a', 'b'] }
  ))
  .register(new ToolDef(
    'get_time', 'Get current time', 
    () => JSON.stringify({ utc: new Date().toISOString() }),
    {}, { type: 'object', properties: {} }
  ));

// Ask with tool use
const result = await agent.react('What is 5 + 3? What is the current time?');
console.log(result);

await brain.stop();
```

---

## @tool() Decorator

Decorator factory for creating tools from class methods.

### Usage

```javascript
import { tool } from 'brainos';

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

// Create instance and extract tool definitions
const instance = new MyTools();
const addTool = instance.add.toolDef;  // Access via .toolDef property
```

### Parameters

- `description` (string): Description of what the tool does
- `options` (object, optional): Additional options
  - `name` (string): Override the function name as tool name

---

## ToolDef

Tool definition class for creating tools that agents can use.

### Constructor

```javascript
new ToolDef(name, description, callback, parameters, schema)
```

Parameters:
- `name` (string): Tool name
- `description` (string): Tool description
- `callback` (Function): Function that executes when tool is called
- `parameters` (object): Parameter definitions for validation
- `schema` (object): JSON Schema for the tool parameters

#### Methods

All properties are accessible directly:
- `name`: Tool name
- `description`: Tool description
- `callback`: Tool callback function
- `parameters`: Parameter definitions
- `schema`: JSON Schema

#### Example

```javascript
import { ToolDef } from 'brainos';

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
```

---

## BusManager

Async context manager for Bus lifecycle.

### Constructor

```javascript
new BusManager(options = {})
```

Options:
- `mode` (string): Bus mode ('peer', 'client', 'server')
- `connect` (string): Connection address for client mode
- `listen` (string): Listen address for server mode
- `peer` (string): Peer address for peer-to-peer mode

### Static Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `BusManager.create(options)` | Create and start BusManager | `Promise<BusManager>` |

### Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `start()` | Initialize bus | `Promise<void>` |
| `stop()` | Shutdown bus | `Promise<void>` |
| `publishText(topic, payload)` | Publish text message | `Promise<void>` |
| `publishJson(topic, data)` | Publish JSON message | `Promise<void>` |
| `createPublisher(topic)` | Create a publisher | `Promise<Publisher>` |
| `createSubscriber(topic)` | Create a subscriber | `Promise<Subscriber>` |
| `createQuery(topic)` | Create a query client | `Promise<Query>` |
| `createQueryable(topic, handler)` | Create a queryable server | `Promise<Queryable>` |
| `createCaller(name)` | Create a caller client | `Promise<Caller>` |
| `createCallable(uri, handler)` | Create a callable server | `Promise<Callable>` |

#### Example

```javascript
import { BusManager } from 'brainos';

const bus = await BusManager.create();
await bus.start();

// Publish messages
await bus.publishText('my/topic', 'hello');
await bus.publishJson('my/topic', { data: 123 });

// Create publisher/subscriber
const pub = await bus.createPublisher('output/topic');
const sub = await bus.createSubscriber('input/topic');

await bus.stop();
```

---

## PublisherWrapper

Message publisher for a specific topic.

### Properties

| Property | Type | Description |
|----------|------|-------------|
| `topic` | `string` | Topic name |

### Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `publishText(payload)` | Publish text message | `Promise<void>` |
| `publishJson(data)` | Publish JSON message | `Promise<void>` |

---

## SubscriberWrapper

Message subscriber with receive methods.

### Properties

| Property | Type | Description |
|----------|------|-------------|
| `topic` | `string` | Topic name |

### Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `recv()` | Receive message (blocking) | `Promise<string>` |
| `recvWithTimeoutMs(ms)` | Receive with timeout | `Promise<string | null>` |
| `recvJsonWithTimeoutMs(ms)` | Receive JSON with timeout | `Promise<any | null>` |
| `run(callback)` | Run callback loop for messages | `Promise<void>` |
| `runJson(callback)` | Run JSON callback loop for messages | `Promise<void>` |

#### Example

```javascript
import { BusManager } from 'brainos';

const bus = await BusManager.create();
await bus.start();

const sub = await bus.createSubscriber('my/topic');
const msg = await sub.recvWithTimeoutMs(5000);
console.log('Received:', msg);

await bus.stop();
```

---

## QueryClient / QueryableServer

Request-response pattern implementation.

### QueryClient Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `queryText(payload)` | Send text query | `Promise<string>` |
| `queryTextTimeoutMs(payload, ms)` | Send text query with timeout | `Promise<string | null>` |

### QueryableServer Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `setHandler(handler)` | Set handler function | `QueryableServer` |
| `start()` | Start server | `Promise<void>` |
| `run(handler)` | Run with handler function | `Promise<void>` |
| `runJson(handler)` | Run with JSON handler function | `Promise<void>` |

#### Example

```javascript
import { BusManager } from 'brainos';

const bus = await BusManager.create();
await bus.start();

// Server
const q = await bus.createQueryable('svc/uppercase', (text) => text.toUpperCase());
await q.start();

// Client
const query = await bus.createQuery('svc/uppercase');
const result = await query.queryText('hello world');  // "HELLO WORLD"
console.log(result);

await bus.stop();
```

---

## CallerClient / CallableServer

RPC pattern implementation.

### CallerClient Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `callText(payload)` | Call remote service | `Promise<string>` |

### CallableServer Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `setHandler(handler)` | Set handler function | `CallableServer` |
| `start()` | Start server | `Promise<void>` |
| `run(handler)` | Run with handler function | `Promise<void>` |
| `runJson(handler)` | Run with JSON handler function | `Promise<void>` |
| `isStarted()` | Check if server is running | `boolean` |

#### Example

```javascript
import { BusManager } from 'brainos';

const bus = await BusManager.create();
await bus.start();

// Server
const srv = await bus.createCallable('svc/echo', (text) => `echo: ${text}`);
await srv.start();

// Client
const caller = await bus.createCaller('svc/echo');
const result = await caller.callText('ping');  // "echo: ping"
console.log(result);

await bus.stop();
```

---

## ConfigLoader

Configuration loader for loading settings from various sources.

### Constructor

```javascript
new ConfigLoader()
```

### Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `discover()` | Auto-discover config files | `ConfigLoader` |
| `addFile(path)` | Add config file | `ConfigLoader` |
| `addDirectory(path)` | Add config directory | `ConfigLoader` |
| `addInline(data)` | Add inline configuration | `ConfigLoader` |
| `reset()` | Reset configuration | `ConfigLoader` |
| `loadSync()` | Load configuration synchronously | `string` (JSON) |
| `reloadSync()` | Reload configuration synchronously | `string` (JSON) |

#### Example

```javascript
import { ConfigLoader } from 'brainos';

const loader = new ConfigLoader();
loader.discover();
loader.addFile('/path/to/config.toml');
loader.addInline({ key: 'value' });

const config = JSON.parse(loader.loadSync());
console.log(config);
```

---

## McpClient

MCP (Model Context Protocol) client for connecting to external tools and services.

### Static Factory Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `McpClient.spawn(command, args)` | Spawn an MCP server process | `Promise<McpClient>` |
| `McpClient.connectHttp(url)` | Connect via HTTP URL | `McpClient` |

### Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `initialize()` | Initialize MCP connection | `Promise<void>` |
| `listTools()` | List available tools | `Promise<Array>` |
| `callTool(name, argsJson)` | Call a tool with JSON args | `Promise<string>` |
| `listPrompts()` | List available prompts | `Promise<Array>` |
| `listResources()` | List available resources | `Promise<Array>` |
| `readResource(uri)` | Read a resource by URI | `Promise<string>` |

#### Example

```javascript
import { McpClient } from 'brainos';

async function main() {
  // Spawn an MCP server process
  const client = await McpClient.spawn('npx', ['-y', '@modelcontextprotocol/server-filesystem', '/tmp']);
  await client.initialize();

  // List available tools
  const tools = await client.listTools();
  console.log('Available tools:', tools);

  // Call a tool
  const result = await client.callTool('read_file', JSON.stringify({ path: '/tmp/test.txt' }));
  console.log(result);

  // List resources
  const resources = await client.listResources();
  console.log(resources);

  // Read a resource
  const resource = await client.readResource('resource://file:///tmp/test.txt');
  console.log(resource);
}

main().catch(console.error);
```

---

## Hook Events

BrainOS supports hooks to intercept and react to events during agent execution.

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
|----------|-------------|
| `'continue'` | Proceed normally (default) |
| `'abort'` | Abort current operation |
| `'error:message'` | Return an error with message |

#### Example

```javascript
import { BrainOS, HookEvent } from 'brainos';

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

await brain.stop();
```
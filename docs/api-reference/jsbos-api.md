# BrainOS JavaScript API Reference

This document provides the complete API reference for the BrainOS JavaScript/Node.js bindings (`brainos` package).

## Main Entry Point

### BrainOS

Main entry point — manages Bus lifecycle, config auto-discovery, and global tool registry.

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
| `start()` | Initialize BrainOS (auto-discovers config) | `Promise<BrainOS>` |
| `stop()` | Shutdown BrainOS | `Promise<void>` |
| `agent(name, options)` | Create an AgentBuilder | `AgentBuilder` |
| `registerGlobal(...tools)` | Register global tools | `BrainOS` |
| `tools(...tools)` | Alias for `registerGlobal` | `BrainOS` |
| `createBus(options)` | Create a BusManager | `Promise<BusManager>` |

#### Static Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `BrainOS.create(options)` | Create and start instance | `Promise<BrainOS>` |

#### Properties

| Property | Type | Description |
|----------|------|-------------|
| `bus` | `BusManager` | The BusManager instance |
| `config` | `Config` | Loaded configuration |
| `registry` | `ToolRegistry` | Global tool registry |
| `isStarted` | `boolean` | Whether BrainOS is started |

#### Example

```javascript
import { BrainOS } from 'brainos';

const brain = new BrainOS();
await brain.start();

const agent = brain.agent('assistant')
  .register(addTool)
  .prompt('You are helpful.');

await brain.stop();
```

---

## AgentBuilder

Fluent builder for creating agents with chainable configuration.

#### Constructor

```javascript
new AgentBuilder(bus, options = {})
```

#### Fluent Configuration Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `name(name)` | Set agent name | `AgentBuilder` |
| `model(model)` | Set model name | `AgentBuilder` |
| `baseUrl(url)` | Set base URL | `AgentBuilder` |
| `apiKey(key)` | Set API key | `AgentBuilder` |
| `system(prompt)` / `prompt(prompt)` | Set system prompt | `AgentBuilder` |
| `temperature(temp)` | Set temperature | `AgentBuilder` |
| `timeout(secs)` | Set timeout | `AgentBuilder` |
| `maxTokens(tokens)` | Set max tokens | `AgentBuilder` |
| `withConfig(config)` | Apply config object | `AgentBuilder` |
| `tools(...tools)` / `register(...tools)` / `withTools(...tools)` | Register tools | `AgentBuilder` |
| `bash(name, workspaceRoot)` | Add bash tool | `AgentBuilder` |
| `circuitBreaker(maxFailures, cooldownSecs)` | Configure circuit breaker | `AgentBuilder` |
| `rateLimit(capacity, windowSecs, maxRetries)` | Configure rate limiter | `AgentBuilder` |
| `resilience(config)` | Configure both resilience features | `AgentBuilder` |
| `hook(event, callback)` | Register a lifecycle hook | `AgentBuilder` |
| `hooks(hooks)` | Register multiple hooks | `AgentBuilder` |
| `plugin(nameOrObj, handlers)` | Register a plugin | `AgentBuilder` |
| `skill(name, content)` | Add inline skill | `AgentBuilder` |
| `skillsFromDir(dirPath)` | Load skills from directory | `AgentBuilder` |
| `mcp(ns, cmd, args)` | Add MCP server (process) | `AgentBuilder` |
| `mcpHttp(ns, url)` | Add MCP server (HTTP) | `AgentBuilder` |

#### Execution Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `start()` | Build and initialize the agent | `Promise<AgentBuilder>` |
| `ask(prompt)` | Auto-start + run simple | `Promise<string>` |
| `runSimple(prompt)` | Auto-start + run simple | `Promise<string>` |
| `react(task)` | Auto-start + run ReAct | `Promise<string>` |
| `stream(task, onToken)` | Stream response tokens | `void` |
| `streamCollect(task)` | Collect all streaming tokens | `Promise<Array>` |
| `stop(options)` | Stop the agent | `object` |
| `isRunning()` | Check if agent is running | `boolean` |

#### Properties

| Property | Type | Description |
|----------|------|-------------|
| `session` | `SessionManager` | Session management |

#### Example

```javascript
const agent = await brain.agent('assistant')
  .name('math-bot')
  .tools(addTool, multiplyTool)
  .prompt('You are a math expert.')
  .temperature(0.5)
  .hook(HookEvent.BeforeToolCall, myHook)
  .bash('bash')
  .start();

const result = await agent.ask('What is 15 + 23?');
```

---

## AgentWrapperClass

High-level agent wrapper. Created via `AgentBuilder.start()`.

#### Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `ask(prompt)` | Run simple task | `Promise<string>` |
| `react(task)` | Run with ReAct reasoning | `Promise<string>` |
| `stream(task, onToken)` | Stream response tokens | `void` |
| `streamCollect(task)` | Collect all streaming tokens | `Promise<Array>` |
| `stop(options)` | Stop the agent | `object` |
| `isRunning()` | Check if running | `boolean` |
| `listMcpTools()` | List MCP tools | `Promise<Array>` |
| `resetMetrics()` | Reset performance metrics | `void` |

#### Properties

| Property | Type | Description |
|----------|------|-------------|
| `session` | `SessionManager` | Session management |
| `tools` | `string[]` | Registered tool names |
| `config` | `object` | Agent configuration |
| `metrics` | `object` | Performance metrics |
| `inner` | `Agent` | Native agent reference |

---

## SessionManager

Session management for an agent.

#### Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `save(path)` | Save message log | `Promise<SessionManager>` |
| `restore(path)` | Restore message log | `Promise<SessionManager>` |
| `saveFull(path)` | Save full session | `Promise<SessionManager>` |
| `restoreFull(path)` | Restore full session | `Promise<SessionManager>` |
| `compact(keepRecent, maxSummaryChars)` | Compact conversation | `SessionManager` |
| `clear()` | Clear session | `SessionManager` |
| `getMessages()` | Get all messages | `Array` |
| `addMessage(role, content)` | Add a message | `SessionManager` |
| `export()` | Export session state | `string` (JSON) |
| `import(json)` | Import session state | `SessionManager` |

---

## ToolDef

Tool definition class for creating tools. Supports both sync and async callbacks (auto-detected via `isPromise`).

#### Constructor

```javascript
new ToolDef(name, description, callback, parameters = {}, schema = {})
```

Parameters:
- `name` (string): Tool name
- `description` (string): Tool description
- `callback` (Function): `(args) => result` — can be sync or return a Promise
- `parameters` (object): Parameter definitions
- `schema` (object): JSON Schema

#### Example - Sync Tool

```javascript
const weatherTool = new ToolDef(
  'get_weather',
  'Get weather for a city',
  (args) => JSON.stringify({ city: args.city, temp: 22 }),
  { city: { type: 'string' } },
  { type: 'object', properties: { city: { type: 'string' } }, required: ['city'] }
);
```

#### Example - Async Tool (Promise)

Async callbacks are automatically detected and awaited.

```javascript
const asyncWeatherTool = new ToolDef(
  'get_weather_async',
  'Get weather from async API',
  async (args) => {
    const response = await fetch(`/api/weather?city=${args.city}`);
    return JSON.stringify(await response.json());
  },
  { city: { type: 'string' } },
  { type: 'object', properties: { city: { type: 'string' } }, required: ['city'] }
);
```

---

## @tool() Decorator

Decorator factory for creating tools from class methods.

```javascript
import { tool } from 'brainos';

class MyTools {
  @tool('Add two numbers')
  add(args) {
    return args.a + args.b;
  }

  @tool('Multiply', { name: 'multiply' })
  multiply(args) {
    return args.a * args.b;
  }
}

const instance = new MyTools();
const addTool = instance.add.toolDef;
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
| `get(name)` | Get tool by name | `ToolDef \| BaseTool` |
| `has(name)` | Check if tool exists | `boolean` |
| `list()` | List tool names | `string[]` |
| `listTools()` | List tool objects | `Array` |
| `listToolDefs()` | List as ToolDef[] | `ToolDef[]` |
| `listByCategory(category)` | Filter by category | `Array` |
| `filter(predicate)` | Filter tools | `ToolRegistry` |
| `merge(other)` | Merge another registry | `ToolRegistry` |
| `size()` | Count tools | `number` |
| `clear()` | Clear all tools | `ToolRegistry` |
| `toJSON()` | Serialize to JSON | `Array` |

---

## ToolResult

```javascript
import { ToolResult } from 'brainos';

// Success
const result = ToolResult.success(data, { key: 'value' });

// Error
const result = ToolResult.error('Something went wrong');
```

---

## BaseTool / FunctionTool

Base class for creating custom tools.

```javascript
import { BaseTool, ToolCategory } from 'brainos';

class MyTool extends BaseTool {
  constructor() {
    super({
      name: 'my-tool',
      description: 'Does something',
      category: ToolCategory.CUSTOM,
    });
  }

  async execute(args) {
    return this.success({ result: 'done' });
  }
}
```

#### FunctionTool

```javascript
import { FunctionTool } from 'brainos';

const tool = FunctionTool.fromFunction(
  (args) => args.a + args.b,
  'add',
  'Add two numbers'
);
```

---

## BusManager

Async manager for Bus lifecycle.

#### Constructor

```javascript
new BusManager(options = {})
```

Options:
- `mode` (string): Bus mode ('peer', 'client', 'server')
- `connect` (string): Connection address
- `listen` (string): Listen address
- `peer` (string): Peer address

#### Static Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `BusManager.create(options)` | Create and start | `Promise<BusManager>` |

#### Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `start()` | Initialize bus | `Promise<BusManager>` |
| `stop()` | Shutdown bus | `Promise<void>` |
| `mode(mode)` | Set mode | `BusManager` |
| `connect(addresses)` | Set connect addresses | `BusManager` |
| `listen(addresses)` | Set listen addresses | `BusManager` |
| `peer(id)` | Set peer ID | `BusManager` |
| `publish(topic, payload, isJson)` | Publish message | `Promise<void>` |
| `publisher(topic)` | Create publisher | `Promise<PublisherWrapper>` |
| `subscriber(topic)` | Create subscriber | `Promise<SubscriberWrapper>` |
| `query(topic)` | Create query client | `Promise<QueryClient>` |
| `queryable(topic, handler)` | Create queryable server | `Promise<QueryableServer>` |
| `caller(name)` | Create caller client | `Promise<CallerClient>` |
| `callable(uri, handler)` | Create callable server | `Promise<CallableServer>` |

#### Properties

| Property | Type | Description |
|----------|------|-------------|
| `bus` | `Bus` | The native Bus instance |

---

## PublisherWrapper

#### Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `publish(payload, isJson)` | Publish message | `Promise<void>` |
| `text(payload)` | Publish text | `Promise<void>` |
| `json(data)` | Publish JSON | `Promise<void>` |

#### Properties

| Property | Type | Description |
|----------|------|-------------|
| `topic` | `string` | Topic name |

---

## SubscriberWrapper

#### Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `recv(timeoutMs)` | Receive message | `Promise<string>` |
| `recvJson(timeoutMs)` | Receive JSON | `Promise<any>` |
| `run(callback)` | Run callback loop | `Promise<void>` |
| `runJson(callback)` | Run JSON callback loop | `Promise<void>` |

#### Async Iterator

```javascript
const sub = await bus.subscriber('my/topic');
for await (const msg of sub) {
  console.log(msg);
}
```

---

## QueryClient / QueryableServer

#### QueryClient

| Method | Description | Returns |
|--------|-------------|---------|
| `ask(payload, timeoutMs)` | Send query | `Promise<string>` |
| `askJson(payload, timeoutMs)` | Send JSON query | `Promise<any>` |

#### QueryableServer

| Method | Description | Returns |
|--------|-------------|---------|
| `handle(handler)` | Set handler | `QueryableServer` |
| `start()` | Start server | `Promise<void>` |
| `run(handler)` | Run with handler | `Promise<void>` |
| `runJson(handler)` | Run with JSON handler | `Promise<void>` |

---

## CallerClient / CallableServer

#### CallerClient

| Method | Description | Returns |
|--------|-------------|---------|
| `call(payload)` | Call remote service | `Promise<string>` |
| `callJson(payload)` | Call with JSON | `Promise<string>` |

#### CallableServer

| Method | Description | Returns |
|--------|-------------|---------|
| `handle(handler)` | Set handler | `CallableServer` |
| `start()` | Start server | `Promise<void>` |
| `run(handler)` | Run with handler | `Promise<void>` |
| `runJson(handler)` | Run with JSON handler | `Promise<void>` |

#### Properties

| Property | Type | Description |
|----------|------|-------------|
| `isStarted` | `boolean` | Whether server is running |

---

## Config

Configuration loader with fluent API.

```javascript
import { Config } from 'brainos';

const config = Config.load();
console.log(config.model);       // from global_model
console.log(config.baseUrl);     // from global_model.base_url
console.log(config.apiKey);      // from global_model.api_key
```

#### Static Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `Config.load(options)` | Discover and load config | `Config` |
| `Config.fromFile(path)` | Load from file | `Config` |
| `Config.fromDirectory(path)` | Load from directory | `Config` |
| `Config.fromInline(data)` | Load from inline data | `Config` |

#### Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `discover()` | Auto-discover config | `Config` |
| `file(path)` | Add config file | `Config` |
| `directory(path)` | Add config directory | `Config` |
| `inline(data)` | Add inline config | `Config` |
| `reset()` | Reset configuration | `Config` |
| `load()` | Load configuration | `Config` |
| `reload()` | Reload configuration | `Config` |
| `get(key, default)` | Get nested value | `any` |
| `isLoaded()` | Check if loaded | `boolean` |
| `toJSON()` | Get config object | `object` |

#### Properties

| Property | Type | Description |
|----------|------|-------------|
| `globalModel` | `object` | Global model config |
| `model` | `string` | Model name |
| `baseUrl` | `string` | Base URL |
| `apiKey` | `string` | API key |
| `bus` | `object` | Bus config |

---

## McpClient

```javascript
import { McpClient } from 'brainos';

// Process-based
const client = await McpClient.spawn('npx', ['-y', 'server-filesystem', '/tmp']);
await client.initialize();

// HTTP
const client = McpClient.connectHttp('http://127.0.0.1:8000/mcp');
await client.initialize();
```

#### Static Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `McpClient.spawn(command, args)` | Spawn MCP server | `Promise<McpClient>` |
| `McpClient.connectHttp(url)` | Connect via HTTP | `McpClient` |

#### Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `initialize()` | Initialize connection | `Promise<void>` |
| `listTools()` | List available tools | `Promise<Array>` |
| `callTool(name, argsJson)` | Call a tool | `Promise<string>` |
| `listPrompts()` | List prompts | `Promise<Array>` |
| `listResources()` | List resources | `Promise<Array>` |
| `readResource(uri)` | Read resource by URI | `Promise<string>` |

---

## Hooks

Hooks support both sync and async callbacks (auto-detected). Register multiple hooks for the same event.

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

#### Hook Decisions

Return a string from hook callback:

| Decision | Description |
|----------|-------------|
| `'continue'` | Proceed normally |
| `'abort'` | Abort operation |
| `'error:message'` | Return error with message |

#### Example - Sync Hook

```javascript
import { BrainOS, HookEvent } from 'brainos';

const brain = await BrainOS.create();

brain.agent('assistant')
  .hook(HookEvent.BeforeToolCall, (ctx) => {
    console.log('[BeforeToolCall]', ctx.data.tool_name);
    return 'continue';
  })
  .hook(HookEvent.AfterToolCall, (ctx) => {
    console.log('[AfterToolCall]', ctx.data.tool_name);
    return 'continue';
  });
```

#### Example - Async Hook

Async hooks are automatically detected and awaited.

```javascript
const asyncBeforeLlmHook = async (ctx) => {
  console.log('[Async Hook:BeforeLlmCall] Checking rate limit...');
  await new Promise(r => setTimeout(r, 10));  // Simulate async check
  console.log('[Async Hook:BeforeLlmCall] Rate limit passed');
  return 'continue';
};

const asyncAfterLlmHook = async (ctx) => {
  console.log('[Async Hook:AfterLlmCall] Logging response...');
  await new Promise(r => setTimeout(r, 10));  // Simulate async logging
  console.log('[Async Hook:AfterLlmCall] Response logged');
  return 'continue';
};

brain.agent('assistant').with_hooks({
  [HookEvent.BeforeLlmCall]: asyncBeforeLlmHook,
  [HookEvent.AfterLlmCall]: asyncAfterLlmHook,
});
```

---

## Token Usage

#### TokenUsage

| Property | Type | Description |
|----------|------|-------------|
| `promptTokens` | `number` | Prompt token count |
| `completionTokens` | `number` | Completion token count |
| `totalTokens` | `number` | Total token count |

#### TokenBudgetReport

| Property | Type | Description |
|----------|------|-------------|
| `status` | `BudgetStatus` | Budget status |
| `usagePercentage` | `number` | Usage percentage |
| `tokenUsage` | `TokenUsage` | Current usage |

#### BudgetStatus

| Status | Description |
|--------|-------------|
| `Normal` | Within budget |
| `Warning` | Approaching limit |
| `Exceeded` | Over budget |
| `Critical` | Critically over budget |

---

## Best Practices

- Use `BrainOS.create()` for one-line initialization
- Use `AgentBuilder` fluent API for agent configuration
- Use `ToolDef` or `@tool` decorator for tool definitions
- Both sync and async callbacks are supported — use async for I/O-bound operations (API calls, database queries)
- Async callbacks are auto-detected via `isPromise()` — no special registration needed
- Register global tools via `brain.registerGlobal()` for reuse across agents
- Use `Config.load()` for environment-specific configuration
- Hooks can also be async — use for rate limiting, logging, request/response enrichment
- Keep one `BusManager` instance per process
- Use `session.saveFull()` / `session.restoreFull()` for conversation persistence
- Use `streamCollect()` for simple token collection without manual iteration

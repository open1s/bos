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
| `apiMode(mode)` | Set the API mode (`chat` or `responses`) | `AgentBuilder` |
| `reasoningEffort(effort)` | Set reasoning effort for reasoning models | `AgentBuilder` |
| `tools(...tools)` / `register(...tools)` / `withTools(...tools)` | Register tools | `AgentBuilder` |
| `bash(name, workspaceRoot)` | Add bash tool | `AgentBuilder` |
| `circuitBreaker(maxFailures, cooldownSecs)` | Configure circuit breaker | `AgentBuilder` |
| `rateLimit(capacity, windowSecs, maxRetries)` | Configure rate limiter | `AgentBuilder` |
| `resilience(config)` | Configure both resilience features | `AgentBuilder` |
| `hook(event, callback)` | Register a lifecycle hook | `AgentBuilder` |
| `hooks(hooks)` | Register multiple hooks | `AgentBuilder` |
| `plugin(nameOrObj, handlers)` | Register a plugin | `AgentBuilder` |
| `plugins(...plugins)` | Register multiple plugins | `AgentBuilder` |
| `skill(name, content)` | Add inline skill | `AgentBuilder` |
| `skillsDir(dirPath)` | Load skills from directory | `AgentBuilder` |
| `mcp(ns, cmd, args)` | Add MCP server (process) | `AgentBuilder` |
| `mcpHttp(ns, url)` | Add MCP server (HTTP) | `AgentBuilder` |
| `withMemory(memory, limit, metadata, autoRemember)` | Recall from a `Memory` store; optionally scope and auto-remember exchanges | `AgentBuilder` |
| `remember(content, metadata)` | Store one memory item | `object` |
| `forget(id)` / `forgetAll()` | Remove one or all memory items | `boolean` |
| `recall(query, limit)` | Recall items, honouring the metadata filter | `Array<object>` |
| `recalledContext(query, limit)` | Recall items formatted for a prompt | `string \| null` |
| `maxSteps(steps)` | Set the maximum ReAct steps | `AgentBuilder` |
| `personaComplete(complete)` | Use the system prompt as the whole prompt | `AgentBuilder` |
| `compactionInputTokens(tokens)` | Cap one compaction request's input | `AgentBuilder` |
| `toolOutputCap(bytes)` | Cap the text one tool result may add to the context | `AgentBuilder` |
| `parallelToolCalls(enabled)` | Run one response's tool calls concurrently | `AgentBuilder` |
| `webSearch(enabled)` | Offer the provider's hosted web-search tool | `AgentBuilder` |
| `contextBudget(tokens)` / `setContextBudget(tokens)` | Set (or with no argument read) the send-side context budget | `AgentBuilder` |
| `skillCatalogue(enabled)` / `setSkillCatalogue(enabled)` | Read or choose catalogue vs on-demand search | `AgentBuilder` |
| `fallback(baseUrl, model, apiKey)` | Append an alternate endpoint tried after a provider failure | `AgentBuilder` |
| `deferTool(name)` | Keep a tool out of the advertised array; it stays callable | `AgentBuilder` |
| `deferToolPrefix(prefix)` | Keep every tool whose name starts with `prefix` out of the array | `AgentBuilder` |
| `isDeferredTool(name)` | Whether a tool is withheld from the advertised array | `boolean` |
| `hasDeferredTools()` | Whether any registered tool is withheld | `boolean` |
| `planTool(enabled)` | Register the `update_plan` tool at start | `AgentBuilder` |

#### Runtime Control Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `planItems()` | Snapshot the working plan | `Array<{text, status}>` |
| `setPlan(items)` | Replace the working plan; an empty list clears it | `AgentBuilder` |
| `clearPlan()` | Forget the whole working plan | `AgentBuilder` |
| `addPlanTool()` | Register the `update_plan` tool | `boolean` |
| `steer(content, ticket)` | Queue a message for the running turn's next step | `boolean` |
| `steerDepth()` | Number of steered messages waiting | `number` |
| `takeUndeliveredSteers()` | Take back steered messages no step delivered | `number[]` |
| `removeTools(names)` | Remove asynchronous tools by name | `number` |
| `addLiveTool(tool)` | Register a tool a running turn sees immediately | `Promise<AgentBuilder>` |
| `removeLiveTools(names)` | Remove live extension tools by name | `number` |
| `toolDefinitions()` | Every registered tool as a full definition | `Array<object>` |
| `modelToolDefinitions()` | The tool array the model is actually offered | `Array<object>` |
| `addLivePromptSection(section)` | Append a live extension prompt fragment | `AgentBuilder` |
| `removeLivePromptSection(section)` | Remove one live extension prompt fragment | `boolean` |
| `registerSkill(name, description, content, path)` | Register an inline skill | `AgentBuilder` |
| `addLiveSkill(name, description, content, path)` | Register a skill a running turn sees immediately | `AgentBuilder` |
| `addLiveSkillsFromDir(dirPath)` | Load a directory into the live skill overlay | `number` |
| `removeLiveSkillsByPaths(paths)` | Remove live skills by directory | `number` |
| `liveSkills()` | Every skill in the live overlay | `Array<object>` |
| `removeSkillsByNames(names)` | Remove skills by name | `number` |
| `listSkills()` | Snapshot every registered skill | `Array<object>` |
| `addGoalTools(store)` | Register `get_goal`/`update_goal` against a `GoalStore` | `number` |
| `addJobTools(jobs, session)` | Register `job_output`/`job_kill` against a `JobRegistry` | `number` |
| `addBashTool(name, workspaceRoot, jobs, session)` | Register a sandboxed bash tool | `Promise<AgentBuilder>` |

#### Execution Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `start()` | Build and initialize the agent | `Promise<AgentBuilder>` |
| `ask(prompt)` | Auto-start + run simple | `Promise<string>` |
| `chat(prompt)` | Alias for ask | `Promise<string>` |
| `runSimple(prompt)` | Auto-start + run simple | `Promise<string>` |
| `react(task)` | Auto-start + run ReAct | `Promise<string>` |
| `stream(task, onToken)` | Stream response tokens | `void` |
| `streamCollect(task)` | Collect all streaming tokens | `Promise<Array>` |
| `stop(options)` | Stop the agent | `object` |
| `isRunning()` | Check if agent is running | `boolean` |
| `compactNow(retainTokens)` | Ask the model to write a context checkpoint now | `Promise<object \| null>` |
| `close()` | Release runtime extensions and stop the agent | `AgentBuilder` |

#### Properties

| Property | Type | Description |
|----------|------|-------------|
| `session` | `SessionManager` | Session management (only after `start()`) |
| `config` | `object` | Resolved config, or the pending builder config before `start()` |
| `toolNames` | `string[]` | Registered tool names, before or after `start()` |
| `metrics` | `object \| null` | Performance metrics (incl. `minWallTimeUs`/`maxWallTimeUs`), or `null` before `start()` |

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
| `listMcpResources(namespace)` | List MCP resources, optionally scoped to one namespace | `Promise<Array>` |
| `listMcpPrompts()` | List MCP prompts | `Promise<Array>` |
| `resetMetrics()` | Reset performance metrics | `void` |
| `planItems()` | Snapshot the working plan | `Array<{text, status}>` |
| `setPlan(items)` | Replace the working plan | `AgentWrapperClass` |
| `clearPlan()` | Forget the whole working plan | `AgentWrapperClass` |
| `addPlanTool()` | Register the `update_plan` tool | `boolean` |
| `steer(content, ticket)` | Queue a message for the running turn | `boolean` |
| `steerDepth()` | Number of steered messages waiting | `number` |
| `takeUndeliveredSteers()` | Take back steered messages no step delivered | `number[]` |
| `deferTool(name)` / `deferToolPrefix(prefix)` | Withhold a tool from the advertised array | `AgentWrapperClass` |
| `isDeferredTool(name)` / `hasDeferredTools()` | Deferral projections | `boolean` |
| `removeTools(names)` | Remove asynchronous tools by name | `number` |
| `addLivePromptSection(section)` / `removeLivePromptSection(section)` | Live prompt fragments | `AgentWrapperClass \| boolean` |
| `registerSkill(name, description, content, path)` | Register an inline skill | `AgentWrapperClass` |
| `removeSkillsByNames(names)` | Remove skills by name | `number` |
| `listSkills()` | Snapshot every registered skill | `Array<object>` |
| `contextBudget()` / `setContextBudget(tokens)` | Read or set the context budget | `number \| AgentWrapperClass` |
| `skillCatalogue()` / `setSkillCatalogue(enabled)` | Read or choose catalogue vs search | `boolean \| AgentWrapperClass` |
| `addGoalTools(store)` | Register `get_goal`/`update_goal` | `number` |
| `addJobTools(jobs, session)` | Register `job_output`/`job_kill` | `number` |
| `addBashTool(name, workspaceRoot, jobs, session)` | Register a sandboxed bash tool | `Promise<AgentWrapperClass>` |

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
| `clear()` | Drop every non-system message and reset the context | `SessionManager` |
| `getMessages()` | Get all messages | `Array` |
| `addMessage(role, content)` | Add a message | `SessionManager` |
| `export()` | Export the session snapshot | `object` |
| `exportJson()` | Export the session snapshot as a JSON string | `string` |
| `importSession(data)` | Import a snapshot object or JSON string | `SessionManager` |
| `import(data)` | Alias for `importSession` (back-compat) | `SessionManager` |
| `setContext(context)` | Replace the opaque session context | `SessionManager` |
| `clearContext()` | Reset the session context to null | `SessionManager` |

#### Properties

| Property | Type | Description |
|----------|------|-------------|
| `context` | `object \| null` | The session context (`null` until set) |

---

## ToolDef

Tool definition class for creating tools. Supports both sync and async callbacks (auto-detected via `isPromise`).

#### Constructor

```javascript
new ToolDef(name, description, callback, parameters = {}, schema = {}, cancelCallback = null, metadata = null)
```

Parameters:
- `name` (string): Tool name
- `description` (string): Tool description
- `callback` (Function): `(args) => result` — can be sync or return a Promise
- `parameters` (object): Parameter definitions
- `metadata` (object, optional): Capability metadata (`version`, `risk`, `side_effects`, `permissions`, `timeout_ms`, `max_output_bytes`)
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

#### Static Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `ToolResult.success(data, metadata)` | Build a successful result | `ToolResult` |
| `ToolResult.error(message, metadata)` | Build a failed result | `ToolResult` |
| `ToolResult.fromResult(result)` | Rebuild from a native result object | `ToolResult` |

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

#### Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `execute(args)` | Run the tool; subclasses must override | `Promise<ToolResult>` |
| `validate(args)` | Accept non-null arguments; override for real checks | `boolean` |
| `success(data, metadata)` | Build a successful result tagged with the tool name | `ToolResult` |
| `failure(error, metadata)` | Build a failed result tagged with the tool name | `ToolResult` |
| `toToolDef()` | Convert to a `ToolDef` bound to `execute` | `ToolDef` |
| `BaseTool.fromFunction(fn, name, description, schema)` | Build a `FunctionTool` from a plain function | `FunctionTool` |

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
| `sessionId` | `string` | Session id shared by this bus's primitives |

---

## PublisherWrapper

#### Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `publish(payload, isJson)` | Publish text, or JSON when `isJson` is true | `Promise<void>` |
| `publishText(payload)` | Publish a UTF-8 text payload | `Promise<void>` |
| `publishJson(data)` | Serialize `data` as JSON and publish it | `Promise<void>` |
| `text(payload)` | Deprecated alias for `publishText` | `Promise<void>` |
| `json(data)` | Deprecated alias for `publishJson` | `Promise<void>` |

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
| `getPrompt(name, arguments)` | Fetch one prompt, with optional JSON arguments | `Promise<object>` |
| `getCapabilities()` | Server capabilities reported at initialization | `Promise<object \| null>` |
| `listResources()` | List resources | `Promise<Array>` |
| `readResource(uri)` | Read resource by URI | `Promise<string>` |
| `restart()` | Restart and re-initialize the connection | `Promise<object>` |
| `healthStatus()` | Connection health snapshot | `object` |

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

## Plugins

Plugins intercept LLM requests/responses, tool calls/results, stream tokens, and turn boundaries. Register handlers with `agent.plugin(name, handlers)` or `agent.plugins(...)`.

| Handler | Argument | Description |
|---------|----------|-------------|
| `onLlmRequest` | request | Rewrite an outgoing request; return it (or `null` to keep it) |
| `onLlmResponse` | response | Rewrite a response |
| `onToolCall` | call | Rewrite a tool call |
| `onToolResult` | result | Rewrite a tool result |
| `onStreamToken` | token | Observe or rewrite one stream token |
| `onTurnStart` | turn | Observe a turn before it starts |
| `onTurnEnd` | turn | Observe a finished turn |

`onStreamToken` receives `{ type, text?, name?, args?, id?, output?, ms?, usage? }`;
`onTurnStart` receives `{ agent, input, metadata }`; `onTurnEnd` receives
`{ agent, output?, error? }`. Returning `null` keeps the original value.

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

## Multimodal Content

`Content`, `ContentPart`, and `Binary` describe a multimodal message so an
agent can send images or audio alongside text. A `Content` instance is
accepted anywhere a prompt string is (`run`, `runSimple`, `react`, `stream`).

### Binary

| Member | Description |
|--------|-------------|
| `new Binary(contentType, source, name = null)` | Build from a `{ type: 'url' or 'base64', data }` source |
| `Binary.fromBase64(contentType, data, name = null)` | Build from base64-encoded bytes |
| `Binary.fromUrl(contentType, url, name = null)` | Build from a URL |
| `binary.isImage()` | Whether `contentType` starts with `image/` |
| `binary.isAudio()` | Whether `contentType` starts with `audio/` |
| `binary.url()` | The raw URL, or a `data:` URL for base64 sources |
| `binary.toJSON()` | `{ content_type, source: { url or base64 }, name? }` |

### ContentPart

| Member | Description |
|--------|-------------|
| `ContentPart.text(text)` | A text part |
| `ContentPart.binary(contentType, data, name = null)` | A base64 binary part |
| `ContentPart.binaryUrl(contentType, url, name = null)` | A URL binary part |
| `ContentPart.image(url, name = null)` | A URL image part typed `image/jpeg` |
| `ContentPart.audio(data, format = 'mp3')` | A base64 audio part |
| `ContentPart.audioUrl(url, format = 'mp3')` | A URL audio part |
| `part.toJSON()` | `{ type: 'text', text }` or `{ type: 'binary', binary }` |

### Content

| Member | Description |
|--------|-------------|
| `Content.text(text)` | Plain text content |
| `Content.parts(parts)` | Content from an array of `ContentPart` |
| `Content.image(url, name = null)` | One URL image part typed `image/jpeg` |
| `Content.audio(data, format = 'mp3')` | One base64 audio part |
| `Content.audioUrl(url, format = 'mp3')` | One URL audio part |
| `content.toJSON()` | A part object for text, or an array of parts |
| `content.toString()` | The JSON string form |
| `content.isMultimodal()` | Whether the content holds binary parts |

## Memory

`Memory` is an in-process, keyword-ranked store that mirrors the Rust
`agent::memory::InMemoryMemory`. Recall earlier facts and re-inject them
into a prompt.

| Member | Description |
|--------|-------------|
| `new Memory()` | Create an empty store |
| `memory.add(content, metadata)` | Store text and return the item object |
| `memory.all()` | Every item, oldest first |
| `memory.search(query, limit, metadata)` | Up to `limit` relevant items, best first; `metadata` keeps exact key/value matches, a blank query returns the most recent |
| `memory.withMaxItems(max_items)` | Cap the store, evicting the oldest items on overflow |
| `memory.maxItems` | The configured cap, or `null` |
| `memory.remove(id)` | Remove by id, returning whether it existed |
| `memory.clear()` | Remove every item |
| `memory.len()` | Number of stored items |
| `memory.isEmpty()` | Whether the store is empty |
| `memory.toJSON()` | Every item, for `JSON.stringify` |
| `memory.recallBlock(query, limit)` | Prompt preamble of the best matches, or `""` |
| `memory.save(path)` | Write every item as JSON lines; returns the count |
| `Memory.load(path)` | New store from a JSON-lines file written by `save` |

## Goal / GoalStore

A goal is the durable objective a session pursues; the plan answers "what steps
are left", the goal answers "what is all of this for". Both are native classes
and mirror the Python binding exactly.

### Goal

| Member | Description |
|--------|-------------|
| `Goal.create(objective, maxRounds)` | Create an active, armed goal (blank objectives throw) |
| `Goal.fromJSON(json)` | Restore a goal from its snapshot |
| `goal.toJSON()` | The goal as a plain object |
| `goal.objective` / `goal.state` / `goal.rounds` / `goal.maxRounds` | Durable fields |
| `goal.activation` / `goal.blockedReason` | Continuation flag and blocker |
| `goal.remaining` / `goal.canAdvance` / `goal.exhausted` | Round-cap projections |
| `goal.advance()` / `goal.pause()` / `goal.resume()` / `goal.complete()` | Phase transitions |
| `goal.block(code, message)` | Stop on a lower-kebab-case policy code |
| `goal.arm()` / `goal.disarm()` / `goal.sanitize()` | Continuation and bounds |

### GoalStore

Seed a store with the session's goal, hand it to `agent.addGoalTools(store)`,
and apply `takeSettle()` when the turn ends.

| Member | Description |
|--------|-------------|
| `new GoalStore(goal)` | Seed a store from a `Goal` |
| `store.snapshot()` | The goal as the model sees it now |
| `store.complete()` / `store.block(code, message)` | Record a settle transition |
| `store.takeSettle()` | Take the recorded transition, if any |
| `store.hasSettle()` | Whether a transition still waits for the host |

## JobRegistry

The live roster of background jobs. Share one instance between the bash tool
that spawns jobs, the job tools that read them, and the host that draws the
list.

| Member | Description |
|--------|-------------|
| `new JobRegistry(jobCap, outputCap)` | Create a bounded roster |
| `jobs.list(session)` | Every job started by `session`, oldest first |
| `jobs.get(id)` | One job's metadata, or `null` |
| `jobs.output(id, maxBytes)` | Captured output, or `null` |
| `jobs.kill(id)` | Stop a running job |
| `jobs.wait(id, timeoutMs)` | Wait for settlement, then return metadata |
| `jobs.runningCount(session)` / `jobs.settledCount(session)` | Roster counts |
| `jobs.clearSettled(session)` | Drop settled jobs |
| `jobs.version()` | Counter that changes on every roster change |

## Ferris SDK

Ferris is the BOS extension framework. These classes bind the SDK's authoring
surface: a JavaScript object supplies the same `start`/`invoke`/`shutdown`
callbacks a Rust `Extension` implements, and `FerrisHarness` runs it in process
exactly like the Rust `TestHarness`.

This is the authoring/runtime harness, not the package manager. Installing,
activating and isolating untrusted packages through the Ferris runtimes is a
separate host surface.

### FerrisManifest

| Member | Description |
|--------|-------------|
| `FerrisManifest.parse(toml)` | Parse and validate an `extension.toml` schema-2.0 document |
| `manifest.id` | The extension id |
| `manifest.runtimeKind` | The runtime kind the manifest asks for |
| `manifest.toToml()` | The canonical TOML document |
| `manifest.toJSON()` | The manifest as a plain object |

### FerrisManifestBuilder

Chainable schema-2.0 manifest builder. Every method returns a new builder.

| Member | Description |
|--------|-------------|
| `new FerrisManifestBuilder(id, name, version, description)` | Start a manifest |
| `host(req)` / `extensionApi(req)` | Compatibility ranges |
| `runtime(kind)` / `runtimeConfig(key, value)` | Runtime selection and opaque config |
| `capability(cap)` | Require one capability |
| `contribution(kind, id, payload)` | Declare one contribution with an optional JSON payload |
| `author` / `license` / `repository` / `homepage` | Identity metadata |
| `dependency(id, requirement)` | Dependency requirement |
| `provide(spec)` / `require(spec)` | Service declarations from JSON specs |
| `maxMemoryMb` / `maxExecutionMs` / `maxConcurrentCalls` / `maxOutputBytes` / `maxTableElements` | Resource ceilings |
| `configurationSchema(path)` / `configurationDefaults(value)` | Configuration block |
| `startup(policy)` | `on-startup`, `on-demand`, `on-workspace-open`, `manual` |
| `build()` | Validate and return a `FerrisManifest` |

```javascript
const manifest = new FerrisManifestBuilder('com.example.echo', 'Echo', '1.0.0', 'Echo')
  .runtime('declarative')
  .capability('agent.tools.register')
  .contribution('tool', 'echo', { description: 'Echo' })
  .startup('on-demand')
  .build();
```

### FerrisHarness

| Member | Description |
|--------|-------------|
| `new FerrisHarness(manifest, root, start, invoke, shutdown)` | Build a harness from a TOML manifest and callbacks |
| `harness.start()` | Run `start(ctx)` and return its contributions |
| `harness.invoke(id, input)` | Invoke a contribution id as a tool |
| `harness.invokeContribution(kind, id, input)` | Invoke with an explicit kind |
| `harness.shutdown()` | Run `shutdown()` |
| `harness.manifest()` | The manifest as a plain object |
| `harness.runtimeKind` / `harness.root` | Runtime kind and package root |

Callbacks receive one plain object and may be sync or async:

```javascript
const harness = new FerrisHarness(
  manifestToml,
  '/tmp/echo',
  async (ctx) => [{ kind: 'tool', id: 'echo', payload: { description: 'Echo' } }],
  async (call) => call.input, // { contribution, input, context }
  async () => {},
);

const contributions = await harness.start();
await harness.invoke('echo', { text: 'hi' });
await harness.shutdown();
```

### FerrisHost

The full Ferris package host. It owns a canonical store, installs packages,
plans them in dependency order, activates the standard runtimes, exposes
contributions as JSON, and forwards invocations. Host activation is explicit:
nothing starts until `apply()`.

| Member | Description |
|--------|-------------|
| `new FerrisHost(root, options)` | Open a host; options: `workspace`, `trust`, `policyMode`, `onContributions`, `onRevoke`, `processSandbox` |
| `host.root` | Canonical store root |
| `host.install(source)` | Install a package tree and return `{ id, version }` |
| `host.installPackage(source, origin)` | Install with `installed`, `development` or `builtin` origin |
| `host.installed()` | Installed `{ id, version }` pairs |
| `host.uninstall(id, purgeData)` | Remove an extension, optionally purging its data |
| `host.versions(id)` / `host.activeVersion(id)` | Version list and active version |
| `host.activate(id, version)` / `host.rollback(id)` | Switch the active version |
| `host.setEnabled(id, enabled)` | Enable or disable without uninstalling |
| `host.clearFailure(id)` / `host.quarantine(id, reason)` | Lifecycle recovery |
| `host.packageRoot(id, version)` | Canonical package directory |
| `host.plan()` | Activation order (dependency first) |
| `host.apply()` | Start every planned package; returns applied contributions keyed by extension |
| `host.reload()` | Unload and re-apply the current plan |
| `host.unload()` | Revoke contributions and shut down every runtime |
| `host.contributions()` | Applied contributions as JSON |
| `host.invoke(extension, kind, id, input)` | Invoke one contribution on a started runtime |
| `host.registerExtension(manifest, root, start, invoke, shutdown)` | Register an in-process JS extension as a linked package |
| `host.subscribeEvents(topics, callback)` / `host.unsubscribeEvents(id)` | Host event subscription and cancellation |
| `host.publishEvent(topic, payload)` | Publish a host event; returns `{ delivered, dropped }` |
| `await host.registerAgentTools(agent)` | Register applied tool contributions on an Agent/AgentBuilder |
| `host.grant(extension, capability, scope, note)` | Persist a capability grant; an unknown scope is an error |
| `host.revokeGrant(extension, capability)` | Remove a grant |
| `host.grants(extension)` | Every grant for one extension |
| `host.auditDegraded()` | First audit persistence failure, or `null` |

Runtimes: declarative, process, MCP, Cordis, Extism and WebAssembly Component.
`builtin` manifests are refused because no host-linked builtin is registered.

```javascript
const { FerrisHost } = require('@open1s/jsbos');

const host = new FerrisHost('/tmp/ferris-store', '/tmp/workspace');
host.install('/path/to/package');
host.plan();
const contributions = await host.apply();
const result = await host.invoke('com.example.echo', 'tool', 'echo', { x: 1 });
await host.unload();
```

## Prompt Templates

| Function | Description |
|----------|-------------|
| `renderTemplate(template, variables)` | Substitute `{{name}}` placeholders from a string object |
| `withMemory(template, memory)` | Place a recalled-memory block, honouring `{{memory}}` |
| `estimateTokens(text)` | Estimate tokens with the agent's own heuristic |

```javascript
import { Goal, GoalStore, JobRegistry, renderTemplate } from '@open1s/jsbos';

const goal = Goal.create('Ship the release');
const store = new GoalStore(goal);
agent.addGoalTools(store);

const jobs = new JobRegistry();
agent.addJobTools(jobs, 'default');

const prompt = renderTemplate('You are {{role}}.', { role: 'a reviewer' });
```

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

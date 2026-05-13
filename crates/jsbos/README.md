# @open1s/jsbos

> BrainOS JavaScript/Node.js bindings — AI agent framework with ReAct engine

High-performance JavaScript bindings for [BrainOS](https://github.com/open1s/bos), a Rust-based AI agent framework implementing the ReAct (Reason + Act) paradigm. Built with [NAPI-RS](https://napi.rs/) for native performance.

## Features

- **ReAct Agent** — Async agent with tool-calling capabilities, streaming responses, and automatic reasoning
- **Message Bus** — Publish/subscribe message bus for inter-agent communication
- **Lifecycle Hooks** — Intercept and modify agent behavior at key points
- **Plugin System** — Extend LLM requests/responses and tool execution
- **MCP Client** — Connect to [Model Context Protocol](https://modelcontextprotocol.io/) servers
- **Bash Tool** — Execute shell commands with workspace isolation
- **Skills** — Register agent capabilities from directory files

## Installation

```bash
npm install @open1s/jsbos
# or
yarn add @open1s/jsbos
```

### Requirements

- Node.js >= 12.22.0 (or Node.js >= 14.17.0)
- Prebuilt binaries available for:
  - macOS (x86_64, arm64)
  - Linux (x86_64, aarch64)
  - Windows (x86_64)

## Quick Start

```javascript
import { Agent, Bus } from '@open1s/jsbos';

async function main() {
  // Create a message bus for inter-agent communication
  const bus = await Bus.create();

  // Create an agent with LLM configuration
  const agent = await Agent.create({
    name: 'assistant',
    model: 'gpt-4',
    baseUrl: 'https://api.openai.com/v1',
    apiKey: process.env.OPENAI_API_KEY,
    systemPrompt: 'You are a helpful assistant.',
    temperature: 0.7,
    timeoutSecs: 120,
  }, bus);

  // Register a custom tool
  await agent.addTool(
    'calculator',
    'Evaluate a mathematical expression',
    JSON.stringify({
      expression: { type: 'string', description: 'Math expression like 2 + 2' }
    }),
    JSON.stringify({
      type: 'object',
      properties: { expression: { type: 'string' } },
      required: ['expression']
    }),
    (err, args) => {
      // Tool implementation
      const result = eval(args.expression);
      return JSON.stringify({ result });
    }
  );

  // Run the agent with a task
  const response = await agent.runSimple('What is 15 * 23?');
  console.log(response);
}

main().catch(console.error);
```

## API Reference

### Agent

The core AI agent class with tool-calling and ReAct reasoning capabilities.

#### `Agent.create(config)` — Create a new agent

```javascript
const agent = await Agent.create({
  name: 'my-agent',           // Agent name
  model: 'gpt-4',             // LLM model identifier
  baseUrl: 'https://api.openai.com/v1',
  apiKey: process.env.OPENAI_API_KEY,
  systemPrompt: 'You are a helpful assistant.',
  temperature: 0.7,           // Sampling temperature (0-2)
  maxTokens: 4096,            // Max tokens in response
  timeoutSecs: 120,           // Request timeout
  maxSteps: 10,               // Max ReAct steps
  // Rate limiting
  rateLimitCapacity: 40,
  rateLimitWindowSecs: 60,
  rateLimitMaxRetries: 3,
  // Circuit breaker
  circuitBreakerMaxFailures: 5,
  circuitBreakerCooldownSecs: 30,
  // Context compaction (for long conversations)
  contextCompactionThresholdTokens: 100000,
  contextCompactionTriggerRatio: 0.8,
  contextCompactionKeepRecentMessages: 10,
  contextCompactionSummaryMaxTokens: 2000,
}, bus);  // Optional: bus for RPC communication
```

#### `agent.runSimple(task)` — Run a simple task

Execute a task with automatic tool calling. Returns the final response string.

```javascript
const response = await agent.runSimple('What is 100 * 100?');
console.log(response); // "100 * 100 = 10000"
```

#### `agent.react(task)` — Run with ReAct reasoning

Execute a task using explicit ReAct (Reason + Act) reasoning loop.

```javascript
const response = await agent.react('Find files modified in the last hour');
```

#### `agent.stream(task, callback)` — Stream responses

Process tasks with streaming token support.

```javascript
await agent.stream('Write a story', (err, token) => {
  if (err) {
    console.error('Error:', token.error);
    return;
  }

  switch (token.type) {
    case 'Text':
      process.stdout.write(token.text);
      break;
    case 'ReasoningContent':
      console.error('[Reasoning]', token.text);
      break;
    case 'ToolCall':
      console.log('[Tool]', token.name, token.args);
      break;
    case 'Done':
      console.log('\n[Complete]');
      break;
  }
});
```

#### `agent.addTool(name, description, parameters, schema, callback)` — Register a tool

Register a JavaScript function as an agent tool.

```javascript
await agent.addTool(
  'weather',
  'Get weather for a city',
  JSON.stringify({
    city: { type: 'string', description: 'City name' }
  }),
  JSON.stringify({
    type: 'object',
    properties: {
      city: { type: 'string', description: 'City name' }
    },
    required: ['city']
  }),
  (err, args) => {
    // args = { city: 'Tokyo' }
    return JSON.stringify({ temp: 22, condition: 'sunny' });
  }
);
```

#### `agent.addBashTool(name, workspaceRoot?)` — Add bash execution

Add a tool that executes shell commands with optional workspace isolation.

```javascript
await agent.addBashTool('bash', '/path/to/workspace');
// Agent can now run: bash { command: "ls -la", cwd: "/path/to/workspace" }
```

#### `agent.registerSkillsFromDir(dirPath)` — Register skills

Load agent skills from a directory (see skills format in main BrainOS crate).

```javascript
await agent.registerSkillsFromDir('./skills');
```

#### `agent.addMcpServer(namespace, command, args)` — Add MCP server

Connect to a local MCP server process.

```javascript
await agent.addMcpServer(
  'filesystem',           // Namespace for tools
  'npx',                  // Command
  ['-y', '@modelcontextprotocol/server-filesystem', '/path/to/dir']
);
```

#### `agent.addMcpServerHttp(namespace, url)` — Connect to HTTP MCP server

Connect to an MCP server over HTTP/SSE.

```javascript
await agent.addMcpServerHttp('mcp-server', 'https://mcp.example.com/sse');
```

#### `agent.listMcpTools()`, `listMcpResources()`, `listMcpPrompts()`

List available MCP tools, resources, and prompts.

```javascript
const tools = await agent.listMcpTools();
const resources = await agent.listMcpResources('filesystem');
const prompts = await agent.listMcpPrompts();
```

#### `agent.registerHook(event, callback)` — Register lifecycle hooks

Intercept agent events and optionally modify behavior.

```javascript
agent.registerHook('BeforeToolCall', (err, ctx) => {
  console.log('About to call:', ctx.data.toolName);
  return 'continue'; // or 'abort' to block
});
```

**Hook Events:**
- `BeforeToolCall` — Before a tool is executed
- `AfterToolCall` — After a tool completes
- `BeforeLlmCall` — Before LLM request
- `AfterLlmCall` — After LLM response
- `OnMessage` — On each message in conversation
- `OnComplete` — When agent finishes
- `OnError` — On error

#### `agent.registerPlugin(...)` — Register a plugin

Add a plugin to intercept LLM requests/responses and tool calls.

```javascript
agent.registerPlugin(
  'my-plugin',
  (err, req) => { /* modify LLM request */ return req; },
  (err, resp) => { /* modify LLM response */ return resp; },
  (err, call) => { /* intercept tool call */ return call; },
  (err, result) => { /* modify tool result */ return result; }
);
```

#### Message Management

```javascript
// Add a message to the conversation
await agent.addMessage({ role: 'user', content: 'Hello' });

// Get all messages
const messages = agent.getMessages();

// Save/restore conversation state
agent.saveMessageLog('./conversation.json');
agent.restoreMessageLog('./conversation.json');

// Session context (arbitrary key-value store)
agent.setSessionContext({ key: 'value' });
const context = agent.sessionContext();
agent.clearSessionContext();

// Save/restore full session
agent.saveSession('./session.json');
agent.restoreSession('./session.json');

// Compact message log for long conversations
agent.compactMessageLog();
```

### Bus (Message Bus)

Distributed message bus for inter-agent and process communication.

#### `Bus.create(config?)` — Create a message bus

```javascript
const bus = await Bus.create({
  mode: 'peer',              // 'peer', 'client', or 'server'
  connect: ['addr1', 'addr2'], // Addresses to connect to
  listen: ['addr1'],          // Addresses to listen on
  peer: 'my-peer-id'          // Peer identifier
});
```

#### `bus.publishText(topic, payload)` / `bus.publishJson(topic, data)`

Publish a message to a topic.

```javascript
await bus.publishText('events', 'Hello subscribers!');
await bus.publishJson('data', { key: 'value' });
```

#### `bus.createPublisher(topic)` — Create a publisher

```javascript
const publisher = await bus.createPublisher('my-topic');
await publisher.publishText('message 1');
await publisher.publishJson({ event: 'data' });
```

#### `bus.createSubscriber(topic)` — Create a subscriber

```javascript
const subscriber = await bus.createSubscriber('my-topic');

// Blocking receive
const msg = await subscriber.recv();
const msg = await subscriber.recvWithTimeoutMs(5000);

// Or run a handler
await subscriber.run((err, msg) => {
  console.log('Received:', msg);
});

// Stop the subscriber
await subscriber.stop();
```

#### `bus.createQuery(topic)` — Request/response queries

```javascript
const query = await bus.createQuery('compute');

// On the responding side:
const queryable = await bus.createQueryable('compute');
await queryable.setHandler(async (err, input) => {
  return JSON.stringify({ result: compute(input) });
});
await queryable.start();

// Make a query:
const response = await query.queryText('calculate 2+2');
// or with timeout:
const response = await query.queryTextTimeoutMs('calculate 2+2', 5000);
```

#### `bus.createCaller(name)` / `bus.createCallable(uri)` — RPC

Remote procedure call support.

```javascript
// Caller
const caller = await bus.createCaller('service-name');
const result = await caller.callText('request data');

// Callable (service)
const callable = await bus.createCallable('my-service');
await callable.setHandler(async (err, input) => {
  return await processRequest(input);
});
await callable.start();
```

### HookRegistry

Standalone hook registry for external use.

```javascript
const registry = new HookRegistry();

await registry.register('BeforeToolCall', (err, ctx) => {
  console.log('Tool call:', ctx.agentId, ctx.data);
  return 'continue';
});
```

### McpClient

Standalone MCP client (not requiring an agent).

```javascript
import { McpClient } from '@open1s/jsbos';

// From command
const client = await McpClient.spawn('npx', ['-y', '@modelcontextprotocol/server-filesystem', '/tmp']);
await client.initialize();

// Or HTTP
const client = McpClient.connectHttp('http://127.0.0.1:8000/mcp');
await client.initialize();

// Or HTTPS
const client = McpClient.connectHttp('https://mcp.example.com/mcp');
await client.initialize();

// Use MCP
const tools = await client.listTools();
const result = await client.callTool('tool-name', JSON.stringify({ arg: 'value' }));
const prompts = await client.listPrompts();
const resources = await client.listResources();
const resource = await client.readResource('resource-uri');
```

### ConfigLoader

Load configuration from files.

```javascript
const loader = new ConfigLoader();
loader.discover();           // Auto-discover config files
loader.addFile('./config.json');
loader.addDirectory('./configs');
loader.addInline({ key: 'value' });

const config = JSON.parse(loader.loadSync());
```

### Logging

```javascript
import { initTracing, logTestMessage } from '@open1s/jsbos';

// Initialize tracing (for debugging)
initTracing();
logTestMessage('Debug message');
```

## Configuration

Create a config file at `~/.bos/conf/config.toml`:

```toml
[global_model]
api_key = "your-api-key"
base_url = "https://api.openai.com/v1"
model = "gpt-4"

[bus]
mode = "peer"
listen = ["127.0.0.1:7890"]
```

Or set environment variables:
- `OPENAI_API_KEY` — API key
- `LLM_BASE_URL` — Base URL (default: NVIDIA API)
- `LLM_MODEL` — Model name

## Examples

See the [examples](./examples/) directory for complete examples:

| Example | Description |
|---------|-------------|
| `agent_demo.js` | Basic agent with tools |
| `agent_mcp_demo.js` | Agent with MCP servers |
| `bus_demo.js` | Pub/sub messaging |
| `caller_demo.js` | RPC pattern |
| `query_demo.js` | Request/response queries |
| `mcp_demo.js` | Standalone MCP client |
| `mcp_http_demo.js` | HTTP MCP connections |
| `agent_stream_demo.js` | Streaming responses |
| `demo-hooks.js` | Lifecycle hooks |
| `demo-plugins.js` | Plugin system |
| `config_demo.js` | Configuration loading |

Run an example:
```bash
node examples/agent_demo.js
```

## Development

```bash
# Install dependencies
yarn

# Build native addon
yarn build

# Build for release
yarn build:release

# Run tests
yarn test

# Format code
yarn format
```

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                        JavaScript/Node.js                    │
├─────────────────────────────────────────────────────────────┤
│  @open1s/jsbos (NAPI-RS bindings)                           │
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

## License

MIT
# @open1s/jsbos

BrainOS JavaScript bindings - AI agent framework with ReAct engine.

## Installation

```bash
npm install @open1s/jsbos
# or
yarn add @open1s/jsbos
```

## What is BrainOS?

BrainOS is a Rust-based AI agent framework implementing the ReAct (Reason + Act) paradigm. It provides:

- **Agent** - Async agent with tool-calling capabilities
- **Bus** - Message bus for inter-agent communication
- **Hooks** - Lifecycle hooks for extensibility
- **Plugin** - LLM plugin system for tool discovery
- **MCP Client** - Model Context Protocol client

## API Overview

### Agent

```typescript
import { Agent, AgentConfig } from '@open1s/jsbos';

const config: AgentConfig = {
  name: 'my-agent',
  tools: [...],
  llm: { provider: 'openai', model: 'gpt-4' }
};

const agent = new Agent(config);
await agent.run('你的任务');
```

### Bus (Message Bus)

```typescript
import { Bus, Session } from '@open1s/jsbos';

const bus = new Bus({ name: 'my-bus' });
const session = await bus.createSession();
session.send({ type: 'message', payload: '...' });
```

### Hooks

```typescript
import { HookRegistry, HookEvent, HookDecision } from '@open1s/jsbos';

const registry = new HookRegistry();
registry.on(HookEvent.BeforeToolCall, async (ctx) => {
  return HookDecision::Allow; // or HookDecision::Block
});
```

### Plugin

```typescript
import { PluginRegistry, PluginToolCall } from '@open1s/jsbos';

const registry = new PluginRegistry();
registry.register('my-plugin', {
  name: 'my-plugin',
  tools: [...]
});
```

### MCP Client

```typescript
import { McpClient } from '@open1s/jsbos';

const client = new McpClient();
await client.connect('mcp-server-name');
```

## Development

```bash
# Install dependencies
yarn

# Build native addon
yarn build

# Run tests
yarn test

# Format code
yarn format
```

## License

MIT
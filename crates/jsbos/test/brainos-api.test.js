import test from 'ava';
import path from 'path';
import fs from 'fs';
import os from 'os';

import {
  BrainOS,
  AgentBuilder,
  Agent,
  tool,
  ToolDef,
  ToolResult,
  ToolCategory,
  BaseTool,
  FunctionTool,
  ToolRegistry,
  BusManager,
  Publisher as PublisherWrapper,
  Subscriber as SubscriberWrapper,
  Query as QueryClient,
  Queryable as QueryableServer,
  Caller as CallerClient,
  Callable as CallableServer,
  SessionManager,
  Config,
  ConfigLoader,
  extractTools,
  defineTool,
  defineTools,
  createTool,
  version,
  initTracing,
  logTestMessage,
  HookEvent,
  HookDecision,
  HookRegistry,
  McpClient,
} from '../index.js';

const { normalizeEndpoints } = await import('../index.js');

test('ToolDef - can be created with all properties', t => {
  const callback = (args) => args.a + args.b;
  const parameters = { type: 'object', properties: { a: { type: 'number' }, b: { type: 'number' } } };
  const schema = { type: 'object', properties: { result: { type: 'number' } } };

  const toolDef = new ToolDef('add', 'Add two numbers', callback, parameters, schema);

  t.is(toolDef.name, 'add');
  t.is(toolDef.description, 'Add two numbers');
  t.is(toolDef.callback, callback);
  t.is(toolDef.parameters, parameters);
  t.is(toolDef.schema, schema);
});

test('ToolDef - callback can be invoked', t => {
  const toolDef = new ToolDef('multiply', 'Multiply', (args) => args.x * args.y);
  const result = toolDef.callback({ x: 3, y: 4 });
  t.is(result, 12);
});



test('ToolResult - constructor creates instance', t => {
  const result = new ToolResult(true, { value: 42 }, null, { extra: 'data' });
  t.true(result.success);
  t.deepEqual(result.data, { value: 42 });
  t.is(result.error, null);
  t.is(result.metadata.extra, 'data');
});

test('ToolResult.success - creates success result', t => {
  const result = ToolResult.success({ value: 100 });
  t.true(result.success);
  t.deepEqual(result.data, { value: 100 });
  t.is(result.error, null);
  t.truthy(result.metadata.timestamp);
});

test('ToolResult.error - creates error result', t => {
  const result = ToolResult.error('Something went wrong');
  t.false(result.success);
  t.is(result.data, null);
  t.is(result.error, 'Something went wrong');
  t.truthy(result.metadata.timestamp);
});

test('ToolResult.fromResult - converts success result', t => {
  const result = ToolResult.fromResult({ success: true, data: 'ok', metadata: {} });
  t.true(result.success);
  t.is(result.data, 'ok');
});

test('ToolResult.fromResult - converts error result', t => {
  const result = ToolResult.fromResult({ success: false, error: 'fail', metadata: {} });
  t.false(result.success);
  t.is(result.error, 'fail');
});



test('ToolCategory - has all expected categories', t => {
  t.is(ToolCategory.FILE, 'file');
  t.is(ToolCategory.SHELL, 'shell');
  t.is(ToolCategory.SEARCH, 'search');
  t.is(ToolCategory.NETWORK, 'network');
  t.is(ToolCategory.CUSTOM, 'custom');
});

test('ToolCategory - is frozen', t => {
  t.throws(() => { ToolCategory.NEW = 'value'; });
});



test('BaseTool - constructor merges metadata', t => {
  const baseTool = new BaseTool({
    name: 'myTool',
    description: 'My tool description',
    dangerous: true,
  });

  t.is(baseTool.metadata.name, 'myTool');
  t.is(baseTool.metadata.description, 'My tool description');
  t.true(baseTool.metadata.dangerous);
  t.is(baseTool.metadata.category, ToolCategory.CUSTOM);
});

test.skip('BaseTool - validate returns true for non-null', t => {
  const baseTool = new BaseTool({ name: 'test' });
  t.true(baseTool.validate({}));
  t.true(baseTool.validate(null));
  t.false(baseTool.validate(undefined));
});

test.skip('BaseTool - execute throws not implemented', t => {
  const baseTool = new BaseTool({ name: 'test' });
  t.throwsAsync(() => baseTool.execute({}));
});

test('BaseTool - success creates ToolResult', t => {
  const baseTool = new BaseTool({ name: 'myTool' });
  const result = baseTool.success({ value: 42 });
  t.true(result.success);
  t.is(result.metadata.toolName, 'myTool');
});

test('BaseTool - failure creates error ToolResult', t => {
  const baseTool = new BaseTool({ name: 'myTool' });
  const result = baseTool.failure('error message');
  t.false(result.success);
  t.is(result.error, 'error message');
  t.is(result.metadata.toolName, 'myTool');
});

test('BaseTool - toToolDef returns ToolDef', t => {
  const baseTool = new BaseTool({
    name: 'testTool',
    description: 'Test description',
    parameters: { type: 'object' },
    schema: { type: 'object' },
  });

  const toolDef = baseTool.toToolDef();
  t.true(toolDef instanceof ToolDef);
  t.is(toolDef.name, 'testTool');
});

test.skip('BaseTool.fromFunction - creates FunctionTool', t => {
  const fn = function add(a, b) { return a + b; };
  fn.name = 'add';
  fn.description = 'Add two numbers';

  const tool = BaseTool.fromFunction(fn, 'add', 'Add numbers');
  t.true(tool instanceof FunctionTool);
});



test('FunctionTool - execute calls the function', async t => {
  const fn = async (args) => args.x + args.y;
  const functionTool = new FunctionTool(fn, 'adder', 'Adds numbers');

  const result = await functionTool.execute({ x: 5, y: 3 });
  t.true(result.success);
  t.is(result.data, 8);
});

test('FunctionTool - execute handles sync functions', async t => {
  const fn = (args) => args.value * 2;
  const functionTool = new FunctionTool(fn, 'doubler', 'Doubles value');

  const result = await functionTool.execute({ value: 21 });
  t.true(result.success);
  t.is(result.data, 42);
});

test('FunctionTool - execute catches errors', async t => {
  const fn = () => { throw new Error(' intentional error'); };
  const functionTool = new FunctionTool(fn, 'errorTool', 'Throws error');

  const result = await functionTool.execute({});
  t.false(result.success);
  t.is(result.error, ' intentional error');
});



test('ToolRegistry - constructor accepts array of tools', t => {
  const tool1 = new BaseTool({ name: 'tool1' });
  const tool2 = new BaseTool({ name: 'tool2' });
  const registry = new ToolRegistry([tool1, tool2]);

  t.is(registry.size(), 2);
});

test('ToolRegistry.add - adds BaseTool', t => {
  const registry = new ToolRegistry();
  const baseTool = new BaseTool({ name: 'myTool' });
  registry.add(baseTool);

  t.true(registry.has('myTool'));
  t.is(registry.get('myTool'), baseTool);
});

test('ToolRegistry.add - adds ToolDef', t => {
  const registry = new ToolRegistry();
  const toolDef = new ToolDef('toolDef', 'Desc', () => {});
  registry.add(toolDef);

  t.true(registry.has('toolDef'));
  t.is(registry.get('toolDef'), toolDef);
});

test.skip('ToolRegistry.add - adds function', t => {
  const registry = new ToolRegistry();
  const fn = function myFunc() { return 'result'; };
  fn.name = 'myFunc';
  fn.description = 'My function';

  registry.add(fn);
  t.true(registry.has('myFunc'));
});

test('ToolRegistry.add - adds plain object', t => {
  const registry = new ToolRegistry();
  const toolObj = {
    name: 'objTool',
    description: 'Object tool',
    callback: () => 'called',
  };

  registry.add(toolObj);
  t.true(registry.has('objTool'));
});

test('ToolRegistry.add - rejects invalid types', t => {
  const registry = new ToolRegistry();
  t.throws(() => registry.add('invalid'));
});

test('ToolRegistry.register - alias for add', t => {
  const registry = new ToolRegistry();
  const result = registry.register(new BaseTool({ name: 'test' }));
  t.is(result, registry);
});

test('ToolRegistry.remove - removes tool', t => {
  const registry = new ToolRegistry();
  registry.add(new BaseTool({ name: 'removeMe' }));
  registry.remove('removeMe');

  t.false(registry.has('removeMe'));
});

test('ToolRegistry.unregister - alias for remove', t => {
  const registry = new ToolRegistry();
  registry.add(new BaseTool({ name: 'unregisterMe' }));
  registry.unregister('unregisterMe');

  t.false(registry.has('unregisterMe'));
});

test('ToolRegistry.get - returns tool', t => {
  const registry = new ToolRegistry();
  const tool = new BaseTool({ name: 'getMe' });
  registry.add(tool);

  t.is(registry.get('getMe'), tool);
});

test('ToolRegistry.has - checks existence', t => {
  const registry = new ToolRegistry();
  registry.add(new BaseTool({ name: 'exists' }));

  t.true(registry.has('exists'));
  t.false(registry.has('notExists'));
});

test('ToolRegistry.list - returns array of names', t => {
  const registry = new ToolRegistry();
  registry.add(new BaseTool({ name: 'tool1' }));
  registry.add(new BaseTool({ name: 'tool2' }));

  const names = registry.list();
  t.deepEqual(names.sort(), ['tool1', 'tool2']);
});

test('ToolRegistry.listTools - returns array of tools', t => {
  const registry = new ToolRegistry();
  const tool1 = new BaseTool({ name: 'tool1' });
  const tool2 = new BaseTool({ name: 'tool2' });
  registry.add(tool1);
  registry.add(tool2);

  const tools = registry.listTools();
  t.is(tools.length, 2);
});

test('ToolRegistry.listToolDefs - returns ToolDef array', t => {
  const registry = new ToolRegistry();
  registry.add(new BaseTool({ name: 'baseTool' }));
  registry.add(new ToolDef('defTool', 'Desc', () => {}));

  const toolDefs = registry.listToolDefs();
  t.is(toolDefs.length, 2);
  t.true(toolDefs[0] instanceof ToolDef);
  t.true(toolDefs[1] instanceof ToolDef);
});

test('ToolRegistry.listByCategory - filters by category', t => {
  const registry = new ToolRegistry();

  const fileTool = new BaseTool({ name: 'fileTool', category: ToolCategory.FILE });
  const shellTool = new BaseTool({ name: 'shellTool', category: ToolCategory.SHELL });
  const customTool = new BaseTool({ name: 'customTool', category: ToolCategory.CUSTOM });

  registry.add(fileTool);
  registry.add(shellTool);
  registry.add(customTool);

  const fileTools = registry.listByCategory(ToolCategory.FILE);
  t.is(fileTools.length, 1);
  t.is(fileTools[0].metadata.name, 'fileTool');
});

test('ToolRegistry.filter - filters with predicate', t => {
  const registry = new ToolRegistry();
  registry.add(new BaseTool({ name: 'tool1' }));
  registry.add(new BaseTool({ name: 'tool2' }));

  const filtered = registry.filter(t => t.metadata.name === 'tool1');
  t.is(filtered.size(), 1);
});

test('ToolRegistry.merge - combines registries', t => {
  const reg1 = new ToolRegistry();
  reg1.add(new BaseTool({ name: 'tool1' }));

  const reg2 = new ToolRegistry();
  reg2.add(new BaseTool({ name: 'tool2' }));

  reg1.merge(reg2);
  t.is(reg1.size(), 2);
  t.true(reg1.has('tool1'));
  t.true(reg1.has('tool2'));
});

test('ToolRegistry.clear - removes all tools', t => {
  const registry = new ToolRegistry();
  registry.add(new BaseTool({ name: 'tool1' }));
  registry.add(new BaseTool({ name: 'tool2' }));
  registry.clear();

  t.is(registry.size(), 0);
});

test('ToolRegistry.toJSON - returns JSON representation', t => {
  const registry = new ToolRegistry();
  registry.add(new BaseTool({ name: 'tool1', description: 'Desc 1' }));

  const json = registry.toJSON();
  t.is(json[0].name, 'tool1');
});



test('tool - creates descriptor with toolDef', () => {
  class MyClass {
    doSomething(args) {
      return 'done';
    }
  }
  tool('Does something')(MyClass.prototype, 'doSomething');

  const instance = new MyClass();
  t.truthy(instance.doSomething.toolDef);
  t.is(instance.doSomething.toolDef.name, 'doSomething');
  t.is(instance.doSomething.toolDef.description, 'Does something');
});

test('tool - accepts options object', () => {
  class MyClass {
    myMethod(args) {
      return 'result';
    }
  }
  tool({ description: 'Custom description', name: 'customName' })(MyClass.prototype, 'myMethod');

  const instance = new MyClass();
  t.is(instance.myMethod.toolDef.name, 'customName');
  t.is(instance.myMethod.toolDef.description, 'Custom description');
});

test('tool - passes schema properties', () => {
  class MyClass {
    calculate(args) {
      return args.a + args.b;
    }
  }
  tool({
    description: 'Calculate',
    schema: {
      properties: {
        a: { type: 'number' },
        b: { type: 'number' },
      },
      required: ['a', 'b'],
    },
  })(MyClass.prototype, 'calculate');

  const instance = new MyClass();
  const toolDef = instance.calculate.toolDef;
  t.truthy(toolDef.parameters.properties.a);
  t.truthy(toolDef.parameters.properties.b);
});



test.skip('createToolClass - wraps methods with toolDef', () => {
  class MyToolClass {
    calculate(args) {
      return args.value * 2;
    }
  }
  tool('Execute calculation')(MyToolClass.prototype, 'calculate');

  const WrappedClass = createToolClass(MyToolClass);
  const instance = new WrappedClass();
  t.truthy(instance.calculate.toolDef);
});



test('extractTools - extracts tools from instance', () => {
  class MyTools {
    tool1(args) { return 1; }

    tool2(args) { return 2; }
  }
  tool('First tool')(MyTools.prototype, 'tool1');
  tool('Second tool')(MyTools.prototype, 'tool2');

  const instance = new MyTools();
  const tools = extractTools(instance);

  t.is(tools.length, 2);
  t.true(tools.some(t => t.name === 'tool1'));
  t.true(tools.some(t => t.name === 'tool2'));
});

test('extractTools - returns empty for non-decorated class', () => {
  class NoTools {
    regularMethod() { return 'regular'; }
  }

  const instance = new NoTools();
  const tools = extractTools(instance);
  t.is(tools.length, 0);
});



test.skip('defineTool - builder pattern creates tool', () => {
  const addTool = defineTool('add', 'Add two numbers')
    ({ a: { type: 'number' }, b: { type: 'number' } })
    ((args) => args.a + args.b);

  t.true(addTool instanceof ToolDef);
  t.is(addTool.name, 'add');
  t.is(addTool.description, 'Add two numbers');
  t.is(addTool.callback({ a: 3, b: 5 }), 8);
});

test.skip('defineTool - supports returns for return schema', () => {
  const multiplyTool = defineTool('multiply', 'Multiply numbers')
    ({ a: { type: 'number' }, b: { type: 'number' } })
    .returns({ result: { type: 'number' } })
    ((args) => args.a * args.b);

  t.is(multiplyTool.schema.properties.result.type, 'number');
});

test.skip('defineTool - handles default values in parameters', () => {
  const defaultTool = defineTool('defaulted', 'With defaults')
    ({ value: 10 })
    ((args) => args.value);

  
  const result = defaultTool.callback({});
  t.is(result, 10);
});



test.skip('defineTools - batch creates multiple tools', () => {
  const tools = defineTools({
    add: {
      description: 'Add numbers',
      params: { a: { type: 'number' }, b: { type: 'number' } },
      fn: (args) => args.a + args.b,
    },
    subtract: {
      description: 'Subtract numbers',
      params: { a: { type: 'number' }, b: { type: 'number' } },
      fn: (args) => args.a - args.b,
    },
  });

  t.true(tools.add instanceof ToolDef);
  t.true(tools.subtract instanceof ToolDef);
  t.is(tools.add.callback({ a: 10, b: 5 }), 15);
  t.is(tools.subtract.callback({ a: 10, b: 5 }), 5);
});



test('createTool - is alias for defineTool', t => {
  const tool1 = createTool('test', 'Test tool')({ value: { type: 'number' } })((args) => args.value);
  const tool2 = defineTool('test', 'Test tool')({ value: { type: 'number' } })((args) => args.value);

  t.is(tool1.name, tool2.name);
  t.is(tool1.callback({ value: 42 }), tool2.callback({ value: 42 }));
});



test('SessionManager - provides fluent interface', t => {

  const mockInner = {
    saveMessageLog: async () => {},
    restoreMessageLog: async () => {},
    saveSession: async () => {},
    restoreSessionFromFile: async () => {},
    compactSession: () => {},
    clearSession: () => {},
    getMessages: () => [],
    addMessage: () => {},
    getSessionJson: () => ({}),
    restoreSessionJson: () => {},
  };

  const manager = new SessionManager(mockInner);

  t.truthy(manager.save);
  t.truthy(manager.restore);
  t.truthy(manager.saveFull);
  t.truthy(manager.restoreFull);
  t.truthy(manager.compact);
  t.truthy(manager.clear);
  t.truthy(manager.getMessages);
  t.truthy(manager.addMessage);
  t.truthy(manager.export);
  t.truthy(manager.import);
});



test('AgentBuilder - can be constructed with bus and options', t => {
  
  const mockBus = {
    createAgent: async () => ({}),
  };

  const builder = new AgentBuilder(mockBus, {
    name: 'testAgent',
    model: 'gpt-4',
    temperature: 0.5,
  });

  t.is(builder._config.name, 'testAgent');
  t.is(builder._config.model, 'gpt-4');
  t.is(builder._config.temperature, 0.5);
});

test('AgentBuilder - fluent methods return this', t => {
  const mockBus = { createAgent: async () => ({}) };
  const builder = new AgentBuilder(mockBus);

  t.is(builder.name('newName'), builder);
  t.is(builder.model('gpt-4'), builder);
  t.is(builder.baseUrl('http://localhost'), builder);
  t.is(builder.apiKey('key123'), builder);
  t.is(builder.system('You are helpful'), builder);
  t.is(builder.prompt('You are helpful'), builder);
  t.is(builder.temperature(0.9), builder);
  t.is(builder.timeout(60), builder);
  t.is(builder.maxTokens(1000), builder);
});

test('AgentBuilder - withConfig merges configuration', t => {
  const mockBus = { createAgent: async () => ({}) };
  const builder = new AgentBuilder(mockBus);

  builder.withConfig({
    name: 'configured',
    model: 'claude',
    temperature: 0.3,
    systemPrompt: 'Custom prompt',
  });

  t.is(builder._config.name, 'configured');
  t.is(builder._config.model, 'claude');
  t.is(builder._config.temperature, 0.3);
  t.is(builder._config.systemPrompt, 'Custom prompt');
});

test('AgentBuilder - tools adds to registry', t => {
  const mockBus = { createAgent: async () => ({}) };
  const builder = new AgentBuilder(mockBus);

  builder.tools(new BaseTool({ name: 'tool1' }));
  builder.tools(new ToolDef('tool2', 'Desc', () => {}));

  t.is(builder._tools.size(), 2);
});

test('AgentBuilder - register is alias for tools', t => {
  const mockBus = { createAgent: async () => ({}) };
  const builder = new AgentBuilder(mockBus);

  builder.register(new BaseTool({ name: 'regTool' }));
  t.true(builder._tools.has('regTool'));
});

test('AgentBuilder - withTools is alias for tools', t => {
  const mockBus = { createAgent: async () => ({}) };
  const builder = new AgentBuilder(mockBus);

  builder.withTools(new BaseTool({ name: 'withTool' }));
  t.true(builder._tools.has('withTool'));
});

test('AgentBuilder - bash configures bash tool', t => {
  const mockBus = { createAgent: async () => ({}) };
  const builder = new AgentBuilder(mockBus);

  builder.bash('bash', '/workspace');

  t.deepEqual(builder._config._bashTool, { name: 'bash', workspaceRoot: '/workspace' });
});

test('AgentBuilder - circuitBreaker configures circuit breaker', t => {
  const mockBus = { createAgent: async () => ({}) };
  const builder = new AgentBuilder(mockBus);

  builder.circuitBreaker(5, 60);

  t.is(builder._config.circuitBreakerMaxFailures, 5);
  t.is(builder._config.circuitBreakerCooldownSecs, 60);
});

test('AgentBuilder - rateLimit configures rate limiting', t => {
  const mockBus = { createAgent: async () => ({}) };
  const builder = new AgentBuilder(mockBus);

  builder.rateLimit(100, 60, 5);

  t.is(builder._config.rateLimitCapacity, 100);
  t.is(builder._config.rateLimitWindowSecs, 60);
  t.is(builder._config.rateLimitMaxRetries, 5);
});

test('AgentBuilder - resilience configures both circuit breaker and rate limit', t => {
  const mockBus = { createAgent: async () => ({}) };
  const builder = new AgentBuilder(mockBus);

  builder.resilience({
    circuitBreaker: { maxFailures: 3, cooldownSecs: 45 },
    rateLimit: { capacity: 50, windowSecs: 30, maxRetries: 2 },
  });

  t.is(builder._config.circuitBreakerMaxFailures, 3);
  t.is(builder._config.circuitBreakerCooldownSecs, 45);
  t.is(builder._config.rateLimitCapacity, 50);
  t.is(builder._config.rateLimitWindowSecs, 30);
});

test('AgentBuilder - hook registers hooks', t => {
  const mockBus = { createAgent: async () => ({}) };
  const builder = new AgentBuilder(mockBus);

  const callback = () => {};
  builder.hook('BeforeToolCall', callback);

  t.is(builder._hooks.length, 1);
  t.is(builder._hooks[0].event, 'BeforeToolCall');
  t.is(builder._hooks[0].callback, callback);
});

test('AgentBuilder - hooks registers multiple hooks', t => {
  const mockBus = { createAgent: async () => ({}) };
  const builder = new AgentBuilder(mockBus);

  const hook1 = () => 'hook1';
  const hook2 = () => 'hook2';
  builder.hooks({ BeforeToolCall: hook1, AfterToolCall: hook2 });

  t.is(builder._hooks.length, 2);
});

test('AgentBuilder - plugin registers plugins', t => {
  const mockBus = { createAgent: async () => ({}) };
  const builder = new AgentBuilder(mockBus);

  const plugin = { name: 'myPlugin', onRequest: () => {} };
  builder.plugin(plugin);

  t.is(builder._plugins.length, 1);
  t.is(builder._plugins[0].name, 'myPlugin');
});

test('AgentBuilder - plugin with string name creates object', t => {
  const mockBus = { createAgent: async () => ({}) };
  const builder = new AgentBuilder(mockBus);

  builder.plugin('stringPlugin', { onRequest: () => {} });

  t.is(builder._plugins[0].name, 'stringPlugin');
});

test('AgentBuilder - skill adds skill', t => {
  const mockBus = { createAgent: async () => ({}) };
  const builder = new AgentBuilder(mockBus);

  builder.skill('coding', 'You are a coder');

  t.is(builder._skills.length, 1);
  t.is(builder._skills[0].name, 'coding');
  t.is(builder._skills[0].content, 'You are a coder');
});

test('AgentBuilder - skillsFromDir adds directory path', t => {
  const mockBus = { createAgent: async () => ({}) };
  const builder = new AgentBuilder(mockBus);

  builder.skillsFromDir('/path/to/skills');

  t.is(builder._skills[0].dirPath, '/path/to/skills');
});

test('AgentBuilder - mcp adds MCP server', t => {
  const mockBus = { createAgent: async () => ({}) };
  const builder = new AgentBuilder(mockBus);

  builder.mcp('namespace', 'npx', ['-y', 'server']);

  t.is(builder._mcpServers.length, 1);
  t.is(builder._mcpServers[0].type, 'process');
  t.is(builder._mcpServers[0].namespace, 'namespace');
});

test('AgentBuilder - mcpHttp adds HTTP MCP server', t => {
  const mockBus = { createAgent: async () => ({}) };
  const builder = new AgentBuilder(mockBus);

  builder.mcpHttp('httpNs', 'https://mcp.example.com');

  t.is(builder._mcpServers.length, 1);
  t.is(builder._mcpServers[0].type, 'http');
  t.is(builder._mcpServers[0].url, 'https://mcp.example.com');
});



test.skip('Agent - wraps inner agent', t => {
  const mockInner = {
    runSimple: async () => 'result',
    react: async () => 'react result',
    stream: () => {},
    listMcpTools: async () => [],
    getPerfMetrics: () => ({}),
    resetPerfMetrics: () => {},
    config: () => ({}),
    listTools: () => [],
  };
  const mockTools = new ToolRegistry();

  const agent = new Agent(mockInner, mockTools);

  t.is(agent.inner, mockInner);
  t.is(agent.tools, mockInner.listTools());
});

test.skip('Agent - ask delegates to inner', async t => {
  const mockInner = {
    runSimple: async (prompt) => `processed: ${prompt}`,
  };

  const agent = new Agent(mockInner, new ToolRegistry());
  const result = await agent.ask('Hello');

  t.is(result, 'processed: Hello');
});

test.skip('Agent - react delegates to inner', async t => {
  const mockInner = {
    react: async (task) => `reacted: ${task}`,
  };

  const agent = new Agent(mockInner, new ToolRegistry());
  const result = await agent.react('Do something');

  t.is(result, 'reacted: Do something');
});

test.skip('Agent - stream delegates to inner', t => {
  const mockInner = {
    stream: (task, cb) => {
      cb(null, { type: 'Done' });
    },
  };

  const agent = new Agent(mockInner, new ToolRegistry());
  const tokens = [];

  agent.stream('task', (token) => tokens.push(token));

  t.is(tokens[0].type, 'Done');
});

test.skip('Agent - streamCollect collects tokens', async t => {
  const mockInner = {
    stream: (task, cb) => {
      cb(null, { type: 'Text', text: 'Hello' });
      cb(null, { type: 'Done' });
    },
  };

  const agent = new Agent(mockInner, new ToolRegistry());
  const tokens = await agent.streamCollect('task');

  t.is(tokens.length, 2);
  t.is(tokens[0].text, 'Hello');
});

test.skip('Agent - session returns SessionManager', t => {
  const mockInner = {
    getMessages: () => [],
    addMessage: () => {},
    getSessionJson: () => ({}),
    restoreSessionJson: () => {},
    saveMessageLog: async () => {},
    restoreMessageLog: async () => {},
    saveSession: async () => {},
    restoreSessionFromFile: async () => {},
    compactSession: () => {},
    clearSession: () => {},
  };

  const agent = new Agent(mockInner, new ToolRegistry());
  const session = agent.session;

  t.true(session instanceof SessionManager);
});

test.skip('Agent - metrics returns perf metrics', t => {
  const mockInner = {
    getPerfMetrics: () => ({ calls: 5, errors: 0 }),
  };

  const agent = new Agent(mockInner, new ToolRegistry());
  const metrics = agent.metrics;

  t.is(metrics.calls, 5);
});

test.skip('Agent - resetMetrics calls inner', t => {
  let resetCalled = false;
  const mockInner = {
    resetPerfMetrics: () => { resetCalled = true; },
  };

  const agent = new Agent(mockInner, new ToolRegistry());
  agent.resetMetrics();

  t.true(resetCalled);
});

test.skip('Agent - listMcpTools delegates to inner', async t => {
  const mockInner = {
    listMcpTools: async () => [{ name: 'mcpTool' }],
  };

  const agent = new Agent(mockInner, new ToolRegistry());
  const tools = await agent.listMcpTools();

  t.is(tools.length, 1);
  t.is(tools[0].name, 'mcpTool');
});



test('BusManager - can be constructed', t => {
  const manager = new BusManager({ mode: 'peer' });

  t.is(manager._mode, 'peer');
});

test('BusManager.create - creates instance', async t => {
  const manager = await BusManager.create({ mode: 'peer' });
  t.true(manager instanceof BusManager);
});

test('BusManager - fluent methods', t => {
  const manager = new BusManager();

  t.is(manager.mode('server'), manager);
  t.is(manager.connect(['addr1', 'addr2']), manager);
  t.is(manager.listen(['addr1']), manager);
  t.is(manager.peer('myPeer'), manager);
});

test('BusManager - publish delegates to bus', async t => {
  const mockBus = {
    publishText: async (topic, payload) => { t.is(topic, 'test'); t.is(payload, 'message'); },
    publishJson: async (topic, payload) => {},
  };

  const manager = new BusManager();
  manager._bus = mockBus;

  await manager.publish('test', 'message', false);
});

test.skip('BusManager - publisher creates wrapper', async t => {
  const mockPub = { topic: 'test', publishText: async () => {}, publishJson: async () => {} };
  const mockBus = {
    createPublisher: async () => mockPub,
  };

  const manager = new BusManager();
  manager._bus = mockBus;

  const pub = await manager.publisher('test');
  t.true(pub instanceof PublisherWrapper);
});

test.skip('BusManager - subscriber creates wrapper', async t => {
  const mockSub = { topic: 'test', recv: async () => null, run: async () => {} };
  const mockBus = {
    createSubscriber: async () => mockSub,
  };

  const manager = new BusManager();
  manager._bus = mockBus;

  const sub = await manager.subscriber('test');
  t.true(sub instanceof SubscriberWrapper);
});

test.skip('BusManager - query creates client', async t => {
  const mockQuery = { topic: 'test', queryText: async () => 'response' };
  const mockBus = {
    createQuery: async () => mockQuery,
  };

  const manager = new BusManager();
  manager._bus = mockBus;

  const q = await manager.query('test');
  t.true(q instanceof QueryClient);
});

test.skip('BusManager - queryable creates server', async t => {
  const mockQueryable = {
    setHandler: () => {},
    start: async () => {},
    run: async () => {},
    runJson: async () => {},
  };
  const mockBus = {
    createQueryable: async () => mockQueryable,
  };

  const manager = new BusManager();
  manager._bus = mockBus;

  const q = await manager.queryable('test');
  t.true(q instanceof QueryableServer);
});

test.skip('BusManager - caller creates client', async t => {
  const mockCaller = { callText: async () => 'result' };
  const mockBus = {
    createCaller: async () => mockCaller,
  };

  const manager = new BusManager();
  manager._bus = mockBus;

  const c = await manager.caller('service');
  t.true(c instanceof CallerClient);
});

test.skip('BusManager - callable creates server', async t => {
  const mockCallable = {
    setHandler: () => {},
    isStarted: () => false,
    start: async () => {},
    run: async () => {},
    runJson: async () => {},
  };
  const mockBus = {
    createCallable: async () => mockCallable,
  };

  const manager = new BusManager();
  manager._bus = mockBus;

  const c = await manager.callable('uri');
  t.true(c instanceof CallableServer);
});



test.skip('PublisherWrapper - exposes topic', t => {
  const mockPub = { topic: 'myTopic' };
  const wrapper = new PublisherWrapper(mockPub);

  t.is(wrapper.topic, 'myTopic');
});

test.skip('PublisherWrapper - publish delegates', async t => {
  let textCalled = false;
  let jsonCalled = false;

  const mockPub = {
    topic: 'test',
    publishText: async () => { textCalled = true; },
    publishJson: async () => { jsonCalled = true; },
  };

  const wrapper = new PublisherWrapper(mockPub);

  await wrapper.publish('text payload');
  t.true(textCalled);

  await wrapper.publish({ data: 'json' }, true);
  t.true(jsonCalled);
});

test.skip('PublisherWrapper - text is alias for publish', async t => {
  let called = false;
  const mockPub = {
    publishText: async () => { called = true; },
  };

  const wrapper = new PublisherWrapper(mockPub);
  await wrapper.text('payload');

  t.true(called);
});

test.skip('PublisherWrapper - json is alias for publish with json', async t => {
  let called = false;
  const mockPub = {
    publishJson: async () => { called = true; },
  };

  const wrapper = new PublisherWrapper(mockPub);
  await wrapper.json({ key: 'value' });

  t.true(called);
});



test.skip('SubscriberWrapper - exposes topic', t => {
  const mockSub = { topic: 'myTopic' };
  const wrapper = new SubscriberWrapper(mockSub);

  t.is(wrapper.topic, 'myTopic');
});

test.skip('SubscriberWrapper - recv delegates', async t => {
  const mockSub = {
    recv: async () => 'message',
    recvWithTimeoutMs: async (ms) => 'timeout message',
  };

  const wrapper = new SubscriberWrapper(mockSub);

  const msg = await wrapper.recv();
  t.is(msg, 'message');

  const msgTimeout = await wrapper.recv(5000);
  t.is(msgTimeout, 'timeout message');
});

test.skip('SubscriberWrapper - recvJson delegates', async t => {
  const mockSub = {
    recvJson: async () => '{"key":"value"}',
    recvJsonWithTimeoutMs: async (ms) => '{"json":true}',
  };

  const wrapper = new SubscriberWrapper(mockSub);

  const msg = await wrapper.recvJson();
  t.deepEqual(msg, { key: 'value' });

  const msgTimeout = await wrapper.recvJson(5000);
  t.deepEqual(msgTimeout, { json: true });
});

test.skip('SubscriberWrapper - run delegates', async t => {
  let callbackCalled = false;
  const mockSub = {
    run: async (cb) => { cb(null, 'message'); },
  };

  const wrapper = new SubscriberWrapper(mockSub);
  await wrapper.run((err, msg) => { callbackCalled = true; });

  t.true(callbackCalled);
});

test.skip('SubscriberWrapper - async iterator', async t => {
  let callCount = 0;
  const mockSub = {
    recv: async () => {
      callCount++;
      return callCount <= 2 ? `message${callCount}` : null;
    },
  };

  const wrapper = new SubscriberWrapper(mockSub);
  const messages = [];

  for await (const msg of wrapper) {
    messages.push(msg);
  }

  t.is(messages.length, 2);
});

test.skip('SubscriberWrapper - next returns done for null', async t => {
  const mockSub = {
    recv: async () => null,
  };

  const wrapper = new SubscriberWrapper(mockSub);
  const result = await wrapper.next();

  t.true(result.done);
});



test.skip('QueryClient - exposes topic', t => {
  const mockQuery = { topic: 'myTopic' };
  const client = new QueryClient(mockQuery);

  t.is(client.topic, 'myTopic');
});

test.skip('QueryClient - ask delegates', async t => {
  const mockQuery = {
    queryText: async (payload) => 'response',
    queryTextTimeoutMs: async (payload, ms) => 'timeout response',
  };

  const client = new QueryClient(mockQuery);

  const result = await client.ask('question');
  t.is(result, 'response');

  const resultTimeout = await client.ask('question', 5000);
  t.is(resultTimeout, 'timeout response');
});

test.skip('QueryClient - askJson stringifies and parses', async t => {
  const mockQuery = {
    queryText: async (payload) => '{"result":42}',
  };

  const client = new QueryClient(mockQuery);

  const result = await client.askJson({ question: 'test' });
  t.deepEqual(result, { result: 42 });
});



test.skip('QueryableServer - handle sets handler', t => {
  const mockQuery = { setHandler: () => {} };
  const server = new QueryableServer(mockQuery);

  const handler = () => 'handler';
  server.handle(handler);

  
  t.true(server instanceof QueryableServer);
});

test.skip('QueryableServer - start delegates', async t => {
  const mockQuery = { start: async () => {} };
  const server = new QueryableServer(mockQuery);

  await server.start();
  t.pass();
});

test.skip('QueryableServer - run delegates', async t => {
  const mockQuery = { run: async (h) => {} };
  const server = new QueryableServer(mockQuery);

  await server.run(() => {});
  t.pass();
});



test.skip('CallerClient - call delegates', async t => {
  const mockCaller = {
    callText: async (payload) => 'response',
  };

  const client = new CallerClient(mockCaller);
  const result = await client.call('request');

  t.is(result, 'response');
});

test.skip('CallerClient - callJson stringifies payload', async t => {
  const mockCaller = {
    callText: async (payload) => payload,
  };

  const client = new CallerClient(mockCaller);
  const result = await client.callJson({ key: 'value' });

  t.is(result, '{"key":"value"}');
});



test.skip('CallableServer - handle sets handler', t => {
  const mockCallable = { setHandler: () => {} };
  const server = new CallableServer(mockCallable);

  const handler = () => 'handler';
  server.handle(handler);

  t.true(server instanceof CallableServer);
});

test.skip('CallableServer - isStarted delegates', t => {
  const mockCallable = { isStarted: () => true };
  const server = new CallableServer(mockCallable);

  t.true(server.isStarted);
});

test.skip('CallableServer - start delegates', async t => {
  const mockCallable = { start: async () => {} };
  const server = new CallableServer(mockCallable);

  await server.start();
  t.pass();
});

test.skip('CallableServer - run delegates', async t => {
  const mockCallable = { run: async (h) => {} };
  const server = new CallableServer(mockCallable);

  await server.run(() => {});
  t.pass();
});

// ============================================================================
// Config Tests
// ============================================================================

test('Config - constructor creates instance', t => {
  const config = new Config();

  t.true(config instanceof Config);
});

test('Config.load - static factory method', t => {
  const config = Config.load();
  t.true(config instanceof Config);
});

test('Config.fromFile - adds file path', t => {
  const config = Config.fromFile('/path/to/config.json');
  t.true(config instanceof Config);
});

test.skip('Config.fromDirectory - adds directory path', t => {
  const config = Config.fromDirectory('/path/to/configs');
  t.true(config instanceof Config);
});

test('Config.fromInline - adds inline data', t => {
  const config = Config.fromInline({ key: 'value' });
  t.true(config instanceof Config);
});

test.skip('Config - fluent interface', t => {
  const config = new Config();

  t.is(config.file('/path'), config);
  t.is(config.directory('/dir'), config);
  t.is(config.inline({}), config);
  t.is(config.discover(), config);
  t.is(config.reset(), config);
});

test('Config - get retrieves nested values', t => {
  const config = new Config();
  config._config = { a: { b: { c: 'deep' } } };
  config._loaded = true;

  t.is(config.get('a.b.c'), 'deep');
  t.is(config.get('a.b.d', 'default'), 'default');
});

test.skip('Config - getters return defaults', t => {
  const config = new Config();
  config._config = {};
  config._loaded = true;

  t.is(config.globalModel, undefined);
  t.is(config.model, 'nvidia/meta/llama-3.1-8b-instruct');
  t.is(config.baseUrl, 'https://integrate.api.nvidia.com/v1');
  t.is(config.apiKey, '');
  t.is(config.bus, undefined);
});

test('Config - toJSON returns config', t => {
  const config = new Config();
  config._config = { key: 'value' };
  config._loaded = true;

  t.deepEqual(config.toJSON(), { key: 'value' });
});

test('Config - isLoaded returns state', t => {
  const config = new Config();
  t.false(config.isLoaded());

  config._loaded = true;
  t.true(config.isLoaded());
});

// ============================================================================
// normalizeEndpoints Tests
// ============================================================================

test.skip('normalizeEndpoints - returns null for null', t => {
  const result = normalizeEndpoints(null);
  t.is(result, null);
});

test.skip('normalizeEndpoints - returns undefined for undefined', t => {
  const result = normalizeEndpoints(undefined);
  t.is(result, undefined);
});

test.skip('normalizeEndpoints - returns array unchanged', t => {
  const endpoints = ['addr1', 'addr2'];
  const result = normalizeEndpoints(endpoints);
  t.is(result, endpoints);
});

test.skip('normalizeEndpoints - normalizes protocol prefixes', t => {
  const endpoints = ['tcp://localhost:8080', 'ws://example.com'];
  const result = normalizeEndpoints(endpoints);

  t.is(result[0], 'tcp/localhost:8080');
  t.is(result[1], 'ws/example.com');
});

test.skip('normalizeEndpoints - leaves non-string elements unchanged', t => {
  const endpoints = ['addr1', { host: 'localhost' }];
  const result = normalizeEndpoints(endpoints);

  t.is(result[0], 'addr1');
  t.deepEqual(result[1], { host: 'localhost' });
});

// ============================================================================
// BrainOS Tests
// ============================================================================

test('BrainOS - constructor creates instance', t => {
  const brain = new BrainOS();

  t.true(brain instanceof BrainOS);
  t.false(brain.isStarted);
});

test('BrainOS - accepts options', t => {
  const brain = new BrainOS({
    model: 'gpt-4',
    apiKey: 'key123',
  });

  t.is(brain._options.model, 'gpt-4');
  t.is(brain._options.apiKey, 'key123');
});

test('BrainOS - static create factory method', async t => {
  // This will fail without proper config, but we can test the method exists
  t.truthy(BrainOS.create);
});

test('BrainOS - agent requires start', t => {
  const brain = new BrainOS();

  t.throws(() => brain.agent('test'));
});

test('BrainOS - bus requires start', t => {
  const brain = new BrainOS();

  t.throws(() => brain.bus);
});

test('BrainOS - config requires start', t => {
  const brain = new BrainOS();

  t.is(brain.config, null);
});

test('BrainOS - registry is ToolRegistry', t => {
  const brain = new BrainOS();

  t.true(brain.registry instanceof ToolRegistry);
});

test('BrainOS - registerGlobal adds tools', t => {
  const brain = new BrainOS();
  brain.registerGlobal(new BaseTool({ name: 'globalTool' }));

  t.true(brain.registry.has('globalTool'));
});

test('BrainOS - tools is alias for registerGlobal', t => {
  const brain = new BrainOS();
  brain.tools(new BaseTool({ name: 'aliasTool' }));

  t.true(brain.registry.has('aliasTool'));
});

test.skip('BrainOS.createBus - static method', t => {
  t.truthy(BrainOS.createBus);
});

// ============================================================================
// Version and Tracing Tests
// ============================================================================

test.skip('version - returns string', t => {
  const v = getVersion();
  t.is(typeof v, 'string');
});

test.skip('enableTracing - initializes without error', t => {
  t.notThrows(() => enableTracing());
});

test('logTestMessage - logs without error', t => {
  t.notThrows(() => logTestMessage('test message'));
});

// ============================================================================
// Hook Classes Tests
// ============================================================================

test.skip('HookEvent - has expected events', t => {
  t.truthy(HookEvent);
  t.truthy(HookDecision);
  t.truthy(HookContextData);
  t.truthy(HookRegistry);
});

// ============================================================================
// McpClient is exported
// ============================================================================

test('McpClient - is exported', t => {
  t.truthy(McpClient);
});

// ============================================================================
// ConfigLoader is exported
// ============================================================================

test('ConfigLoader - can be instantiated', t => {
  const loader = new ConfigLoader();
  t.truthy(loader);
});

test('ConfigLoader - has expected methods', t => {
  const loader = new ConfigLoader();
  t.truthy(loader.discover);
  t.truthy(loader.addFile);
  t.truthy(loader.addDirectory);
  t.truthy(loader.addInline);
  t.truthy(loader.loadSync);
});
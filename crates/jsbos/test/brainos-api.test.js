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
  PublisherWrapper,
  SubscriberWrapper,
  QueryClient,
  QueryableServer,
  CallerClient,
  CallableServer,
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












test('extractTools - returns empty for non-decorated class', t => {
  class NoTools {
    regularMethod() { return 'regular'; }
  }

  const instance = new NoTools();
  const tools = extractTools(instance);
  t.is(tools.length, 0);
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


test('Config.fromInline - adds inline data', t => {
  const config = Config.fromInline({ key: 'value' });
  t.true(config instanceof Config);
});


test('Config - get retrieves nested values', t => {
  const config = new Config();
  config._config = { a: { b: { c: 'deep' } } };
  config._loaded = true;

  t.is(config.get('a.b.c'), 'deep');
  t.is(config.get('a.b.d', 'default'), 'default');
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


// ============================================================================
// Version and Tracing Tests
// ============================================================================



test('logTestMessage - logs without error', t => {
  t.notThrows(() => logTestMessage('test message'));
});

// ============================================================================
// Hook Classes Tests
// ============================================================================


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
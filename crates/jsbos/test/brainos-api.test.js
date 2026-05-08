const { 
  BrainOS,
  AgentBuilder,
  Agent,
  ToolDef,
  ToolResult,
  ToolCategory,
  BaseTool,
  FunctionTool,
  ToolRegistry,
  BusManager,
  SessionManager,
  tool,
} = require('../brainos.js');

async function runTests() {
  let passed = 0;
  let failed = 0;

  function test(name, fn) {
    try {
      fn();
      console.log(`✓ ${name}`);
      passed++;
    } catch (e) {
      console.log(`✗ ${name}`);
      console.log(`  Error: ${e.message}`);
      failed++;
    }
  }

  function assert(condition, message = 'Assertion failed') {
    if (!condition) throw new Error(message);
  }

  function assertEqual(actual, expected, message = '') {
    if (actual !== expected) {
      throw new Error(`${message || 'Assertion failed'}: expected ${expected}, got ${actual}`);
    }
  }

  console.log('\n========== ToolDef Tests ==========\n');
  
  test('ToolDef creates with correct properties', () => {
    const td = new ToolDef('test', 'Test tool', () => {}, { x: { type: 'number' } }, { type: 'object' });
    assertEqual(td.name, 'test');
    assertEqual(td.description, 'Test tool');
    assert(typeof td.callback === 'function');
    assertEqual(td.parameters.x.type, 'number');
  });

  test('ToolDef default parameters', () => {
    const td = new ToolDef('test', 'desc', () => {});
    assertEqual(Object.keys(td.parameters).length, 0);
    assertEqual(Object.keys(td.schema).length, 0);
  });

  console.log('\n========== ToolResult Tests ==========\n');

  test('ToolResult.success creates successful result', () => {
    const result = ToolResult.success({ value: 42 });
    assertEqual(result.success, true);
    assertEqual(result.data.value, 42);
    assertEqual(result.error, null);
    assert(typeof result.metadata.timestamp === 'number');
  });

  test('ToolResult.error creates error result', () => {
    const result = ToolResult.error('Something went wrong');
    assertEqual(result.success, false);
    assertEqual(result.error, 'Something went wrong');
    assertEqual(result.data, null);
  });

  test('ToolResult.success with metadata', () => {
    const result = ToolResult.success({ data: 1 }, { source: 'test' });
    assertEqual(result.metadata.source, 'test');
    assertEqual(result.metadata.timestamp > 0, true);
  });

  console.log('\n========== ToolCategory Tests ==========\n');

  test('ToolCategory has correct values', () => {
    assertEqual(ToolCategory.FILE, 'file');
    assertEqual(ToolCategory.SHELL, 'shell');
    assertEqual(ToolCategory.SEARCH, 'search');
    assertEqual(ToolCategory.NETWORK, 'network');
    assertEqual(ToolCategory.CUSTOM, 'custom');
  });

  test('ToolCategory is frozen', () => {
    try {
      ToolCategory.FILE = 'changed';
      throw new Error('Should have thrown');
    } catch (e) {
      assertEqual(ToolCategory.FILE, 'file');
    }
  });

  console.log('\n========== BaseTool Tests ==========\n');

  test('BaseTool creates with metadata', () => {
    class MyTool extends BaseTool {
      constructor() {
        super({ name: 'mytool', description: 'My tool', category: ToolCategory.FILE });
      }
      async execute() { return this.success({ done: true }); }
    }
    const tool = new MyTool();
    assertEqual(tool.metadata.name, 'mytool');
    assertEqual(tool.metadata.description, 'My tool');
    assertEqual(tool.metadata.category, 'file');
  });

  test('BaseTool.success helper', async () => {
    class MyTool extends BaseTool {
      constructor() { super({ name: 'test' }); }
      async execute(args) { return this.success({ result: args.value * 2 }); }
    }
    const tool = new MyTool();
    const result = await tool.execute({ value: 21 });
    assertEqual(result.success, true);
    assertEqual(result.data.result, 42);
    assertEqual(result.metadata.toolName, 'test');
  });

  test('BaseTool.failure helper', async () => {
    class MyTool extends BaseTool {
      constructor() { super({ name: 'test' }); }
      async execute(args) {
        if (!args.value) return this.failure('value required');
        return this.success({ result: args.value });
      }
    }
    const tool = new MyTool();
    const result = await tool.execute({});
    assertEqual(result.success, false);
    assertEqual(result.error, 'value required');
  });

  test('BaseTool.toToolDef converts correctly', () => {
    class MyTool extends BaseTool {
      constructor() {
        super({ 
          name: 'convert', 
          description: 'Convert tool',
          parameters: { x: { type: 'number' } },
          schema: { type: 'object' }
        });
      }
      async execute() { return this.success({}); }
    }
    const tool = new MyTool();
    const td = tool.toToolDef();
    assertEqual(td.name, 'convert');
    assertEqual(td.description, 'Convert tool');
    assertEqual(td.parameters.x.type, 'number');
    assert(typeof td.callback === 'function');
  });

  test('BaseTool.validate default implementation', () => {
    const tool = new BaseTool({ name: 'test' });
    assertEqual(tool.validate(null), false);
    assertEqual(tool.validate(undefined), false);
    assertEqual(tool.validate({}), true);
    assertEqual(tool.validate(0), true);
  });

  console.log('\n========== FunctionTool Tests ==========\n');

  test('FunctionTool creates from function', () => {
    const fn = (args) => args.x + args.y;
    const tool = new FunctionTool(fn, 'add', 'Add numbers', { x: {}, y: {} });
    assertEqual(tool.metadata.name, 'add');
    assertEqual(tool.metadata.description, 'Add numbers');
  });

  test('FunctionTool.fromFunction helper', () => {
    const addFn = (args) => args.a + args.b;
    const tool = FunctionTool.fromFunction(addFn, 'add', 'Add');
    assertEqual(tool.metadata.name, 'add');
  });

  test('FunctionTool.execute returns success', async () => {
    const tool = new FunctionTool(
      (args) => args.x + args.y,
      'add',
      'Add'
    );
    const result = await tool.execute({ x: 10, y: 32 });
    assertEqual(result.success, true);
    assertEqual(result.data, 42);
  });

  test('FunctionTool.execute catches errors', async () => {
    const tool = new FunctionTool(
      () => { throw new Error('Intentional error'); },
      'failing',
      'Failing tool'
    );
    const result = await tool.execute({});
    assertEqual(result.success, false);
    assertEqual(result.error, 'Intentional error');
  });

  console.log('\n========== ToolRegistry Tests ==========\n');

  test('ToolRegistry creates empty', () => {
    const reg = new ToolRegistry();
    assertEqual(reg.size(), 0);
    assertEqual(reg.list().length, 0);
  });

  test('ToolRegistry.add accepts ToolDef', () => {
    const reg = new ToolRegistry();
    reg.add(new ToolDef('t1', 'desc', () => {}));
    assertEqual(reg.size(), 1);
    assertEqual(reg.has('t1'), true);
    assertEqual(reg.has('t2'), false);
  });

  test('ToolRegistry.add accepts BaseTool', () => {
    class MyTool extends BaseTool {
      constructor() { super({ name: 'mytool', description: 'My tool' }); }
      async execute() { return this.success({}); }
    }
    const reg = new ToolRegistry();
    reg.add(new MyTool());
    assertEqual(reg.size(), 1);
    assertEqual(reg.get('mytool') instanceof MyTool, true);
  });

  test('ToolRegistry.add accepts function', () => {
    const reg = new ToolRegistry();
    const myFunc = (args) => args.x;
    myFunc.name = 'myFunc';
    reg.add(myFunc);
    assertEqual(reg.size(), 1);
  });

  test('ToolRegistry.add is chainable', () => {
    const reg = new ToolRegistry()
      .add(new ToolDef('t1', '', () => {}))
      .add(new ToolDef('t2', '', () => {}));
    assertEqual(reg.size(), 2);
  });

  test('ToolRegistry.register alias', () => {
    const reg = new ToolRegistry();
    reg.register(new ToolDef('t1', '', () => {}));
    assertEqual(reg.size(), 1);
  });

  test('ToolRegistry.remove removes tool', () => {
    const reg = new ToolRegistry();
    reg.add(new ToolDef('t1', '', () => {}));
    reg.add(new ToolDef('t2', '', () => {}));
    assertEqual(reg.size(), 2);
    reg.remove('t1');
    assertEqual(reg.size(), 1);
    assertEqual(reg.has('t1'), false);
  });

  test('ToolRegistry.listToolDefs returns ToolDef array', async () => {
    class MyTool extends BaseTool {
      constructor() { super({ name: 'base', description: 'Base' }); }
      async execute() { return this.success({}); }
    }
    const reg = new ToolRegistry()
      .add(new MyTool())
      .add(new ToolDef('func', 'Function', () => {}));
    
    const toolDefs = reg.listToolDefs();
    assertEqual(toolDefs.length, 2);
    assert(toolDefs[0] instanceof ToolDef);
    assert(toolDefs[1] instanceof ToolDef);
    assertEqual(toolDefs[0].name, 'base');
    assertEqual(toolDefs[1].name, 'func');
  });

  test('ToolRegistry.listByCategory filters correctly', async () => {
    class FileTool extends BaseTool {
      constructor(name) { super({ name, category: ToolCategory.FILE, description: '' }); }
      async execute() { return this.success({}); }
    }
    class ShellTool extends BaseTool {
      constructor(name) { super({ name, category: ToolCategory.SHELL, description: '' }); }
      async execute() { return this.success({}); }
    }
    const reg = new ToolRegistry()
      .add(new FileTool('file1'))
      .add(new ShellTool('shell1'))
      .add(new FileTool('file2'));
    
    const files = reg.listByCategory(ToolCategory.FILE);
    assertEqual(files.length, 2);
    const shells = reg.listByCategory(ToolCategory.SHELL);
    assertEqual(shells.length, 1);
  });

  test('ToolRegistry.filter creates new registry', () => {
    const reg = new ToolRegistry()
      .add(new ToolDef('a', '', () => {}))
      .add(new ToolDef('b', '', () => {}));
    
    const filtered = reg.filter(t => t.name.startsWith('a'));
    assertEqual(filtered.size(), 1);
    assertEqual(reg.size(), 2);
  });

  test('ToolRegistry.merge combines registries', () => {
    const reg1 = new ToolRegistry().add(new ToolDef('t1', '', () => {}));
    const reg2 = new ToolRegistry().add(new ToolDef('t2', '', () => {})).add(new ToolDef('t3', '', () => {}));
    reg1.merge(reg2);
    assertEqual(reg1.size(), 3);
  });

  test('ToolRegistry.clear empties registry', () => {
    const reg = new ToolRegistry()
      .add(new ToolDef('t1', '', () => {}))
      .add(new ToolDef('t2', '', () => {}));
    reg.clear();
    assertEqual(reg.size(), 0);
  });

  test('ToolRegistry.toJSON exports metadata', () => {
    const reg = new ToolRegistry()
      .add(new ToolDef('tool1', 'Description 1', () => {}));
    const json = reg.toJSON();
    assertEqual(json.length, 1);
    assertEqual(json[0].name, 'tool1');
  });

  console.log('\n========== AgentBuilder Tests ==========\n');

  test('AgentBuilder creates with defaults', () => {
    const builder = new AgentBuilder(null, { name: 'assistant' });
    assertEqual(builder._config.name, 'assistant');
    assertEqual(builder._config.model, 'nvidia/meta/llama-3.1-8b-instruct');
    assertEqual(builder._config.temperature, 0.7);
    assertEqual(builder._config.timeoutSecs, 120);
  });

  test('AgentBuilder.name sets name', () => {
    const builder = new AgentBuilder(null).name('myagent');
    assertEqual(builder._config.name, 'myagent');
  });

  test('AgentBuilder.model sets model', () => {
    const builder = new AgentBuilder(null).model('gpt-4');
    assertEqual(builder._config.model, 'gpt-4');
  });

  test('AgentBuilder.system sets prompt', () => {
    const builder = new AgentBuilder(null).system('You are a coding assistant');
    assertEqual(builder._config.systemPrompt, 'You are a coding assistant');
  });

  test('AgentBuilder.prompt alias works', () => {
    const builder = new AgentBuilder(null).prompt('Custom prompt');
    assertEqual(builder._config.systemPrompt, 'Custom prompt');
  });

  test('AgentBuilder.temperature sets temp', () => {
    const builder = new AgentBuilder(null).temperature(1.0);
    assertEqual(builder._config.temperature, 1.0);
  });

  test('AgentBuilder.timeout sets timeout', () => {
    const builder = new AgentBuilder(null).timeout(300);
    assertEqual(builder._config.timeoutSecs, 300);
  });

  test('AgentBuilder.maxTokens sets tokens', () => {
    const builder = new AgentBuilder(null).maxTokens(4096);
    assertEqual(builder._config.maxTokens, 4096);
  });

  test('AgentBuilder.tools adds tools', () => {
    const tool1 = new ToolDef('t1', '', () => {});
    const tool2 = new ToolDef('t2', '', () => {});
    const builder = new AgentBuilder(null).tools(tool1, tool2);
    assertEqual(builder._tools.size(), 2);
  });

  test('AgentBuilder.tools accepts ToolRegistry', () => {
    const reg = new ToolRegistry().add(new ToolDef('t1', '', () => {}));
    const builder = new AgentBuilder(null).tools(reg);
    assertEqual(builder._tools.size(), 1);
  });

  test('AgentBuilder.register alias works', () => {
    const builder = new AgentBuilder(null).register(new ToolDef('t1', '', () => {}));
    assertEqual(builder._tools.size(), 1);
  });

  test('AgentBuilder.bash configures bash tool', () => {
    const builder = new AgentBuilder(null).bash('sh', '/workspace');
    assertEqual(builder._config._bashTool.name, 'sh');
    assertEqual(builder._config._bashTool.workspaceRoot, '/workspace');
  });

  test('AgentBuilder.circuitBreaker configures CB', () => {
    const builder = new AgentBuilder(null).circuitBreaker(10, 60);
    assertEqual(builder._config.circuitBreakerMaxFailures, 10);
    assertEqual(builder._config.circuitBreakerCooldownSecs, 60);
  });

  test('AgentBuilder.rateLimit configures RL', () => {
    const builder = new AgentBuilder(null).rateLimit(100, 30, 5);
    assertEqual(builder._config.rateLimitCapacity, 100);
    assertEqual(builder._config.rateLimitWindowSecs, 30);
    assertEqual(builder._config.rateLimitMaxRetries, 5);
  });

  test('AgentBuilder.resilience sets both CB and RL', () => {
    const builder = new AgentBuilder(null).resilience({
      circuitBreaker: { maxFailures: 5, cooldownSecs: 45 },
      rateLimit: { capacity: 50, windowSecs: 60, maxRetries: 3 }
    });
    assertEqual(builder._config.circuitBreakerMaxFailures, 5);
    assertEqual(builder._config.circuitBreakerCooldownSecs, 45);
    assertEqual(builder._config.rateLimitCapacity, 50);
    assertEqual(builder._config.rateLimitWindowSecs, 60);
    assertEqual(builder._config.rateLimitMaxRetries, 3);
  });

  test('AgentBuilder.hook adds hook', () => {
    const cb = () => 'continue';
    const builder = new AgentBuilder(null).hook('BeforeToolCall', cb);
    assertEqual(builder._hooks.length, 1);
    assertEqual(builder._hooks[0].event, 'BeforeToolCall');
    assertEqual(builder._hooks[0].callback, cb);
  });

  test('AgentBuilder.hooks adds multiple hooks', () => {
    const builder = new AgentBuilder(null).hooks({
      BeforeToolCall: () => 'continue',
      AfterToolCall: () => 'continue'
    });
    assertEqual(builder._hooks.length, 2);
  });

  test('AgentBuilder.mcp adds MCP server', () => {
    const builder = new AgentBuilder(null).mcp('fs', 'npx', ['-y', 'server-fs', '/tmp']);
    assertEqual(builder._mcpServers.length, 1);
    assertEqual(builder._mcpServers[0].namespace, 'fs');
    assertEqual(builder._mcpServers[0].type, 'process');
  });

  test('AgentBuilder.mcpHttp adds HTTP MCP', () => {
    const builder = new AgentBuilder(null).mcpHttp('remote', 'https://example.com/mcp');
    assertEqual(builder._mcpServers.length, 1);
    assertEqual(builder._mcpServers[0].type, 'http');
    assertEqual(builder._mcpServers[0].url, 'https://example.com/mcp');
  });

  console.log('\n========== BusManager Tests ==========\n');

  test('BusManager creates with options', () => {
    const bus = new BusManager({ mode: 'server', listen: ['127.0.0.1:3000'] });
    assertEqual(bus._mode, 'server');
    assertEqual(bus._listen[0], '127.0.0.1:3000');
  });

  test('BusManager.chainable config', () => {
    const bus = new BusManager()
      .mode('client')
      .connect(['server:3000'])
      .peer('my-peer')
      .listen(['0.0.0.0:8080']);
    assertEqual(bus._mode, 'client');
    assertEqual(bus._connect[0], 'server:3000');
    assertEqual(bus._peer, 'my-peer');
    assertEqual(bus._listen[0], '0.0.0.0:8080');
  });

  console.log('\n========== BrainOS Tests ==========\n');

  test('BrainOS creates with options', () => {
    const brain = new BrainOS({ apiKey: 'test-key', model: 'gpt-4' });
    assertEqual(brain._options.apiKey, 'test-key');
    assertEqual(brain._options.model, 'gpt-4');
    assertEqual(brain.isStarted, false);
  });

  test('BrainOS.registry provides ToolRegistry', () => {
    const brain = new BrainOS();
    assert(brain.registry instanceof ToolRegistry);
  });

  test('BrainOS.registerGlobal adds to registry', () => {
    const brain = new BrainOS();
    brain.registerGlobal(new ToolDef('tool', 'desc', () => {}));
    assertEqual(brain.registry.size(), 1);
  });

  test('BrainOS.tools is alias for registerGlobal', () => {
    const brain = new BrainOS();
    brain.tools(new ToolDef('t1', '', () => {})).tools(new ToolDef('t2', '', () => {}));
    assertEqual(brain.registry.size(), 2);
  });

  console.log('\n========== @tool Decorator Tests ==========\n');

  test('@tool decorator creates toolDef', () => {
    const method = function add(args) { return { result: args.a + args.b }; };
    const desc = { value: method };
    const result = tool('Add numbers')(class {}, 'add', desc);
    const td = result.value.toolDef;
    assert(td instanceof ToolDef);
    assertEqual(td.name, 'add');
    assertEqual(td.description, 'Add numbers');
  });

  test('@tool decorator with custom name option', () => {
    const method = function mult(args) { return args.x * 2; };
    const desc = { value: method };
    const result = tool('Multiply', { name: 'multiply' })(class {}, 'mult', desc);
    assertEqual(result.value.toolDef.name, 'multiply');
  });

  test('@tool callback applies defaults', () => {
    const method = function calc(args) { return args.a + args.b; };
    const desc = { value: method };
    const opts = { schema: { properties: { a: { type: 'number', default: 5 }, b: { type: 'number', default: 5 } } } };
    const result = tool('Calc', opts)(class {}, 'calc', desc);
    const wrapped = result.value.toolDef.callback;
    const sum = wrapped({});
    assertEqual(sum, 10);
  });

  console.log('\n========== Summary ==========\n');
  console.log(`Total: ${passed + failed} tests`);
  console.log(`Passed: ${passed}`);
  console.log(`Failed: ${failed}`);
  console.log('');

  if (failed > 0) {
    process.exit(1);
  }
}

runTests().catch(console.error);
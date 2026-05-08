const {
  Bus,
  ConfigLoader,
  Query,
  Queryable,
  Caller,
  Callable,
  Publisher,
  Subscriber,
  McpClient,
  version: getVersion,
  initTracing: enableTracing,
  logTestMessage,
  HookEvent,
  HookDecision,
  HookContextData,
  HookRegistry,
} = require('./index.js');

const DEFAULT_MODEL = 'nvidia/meta/llama-3.1-8b-instruct';
const DEFAULT_BASE_URL = 'https://integrate.api.nvidia.com/v1';

class ToolDef {
  constructor(name, description, callback, parameters = {}, schema = {}) {
    this.name = name;
    this.description = description;
    this.callback = callback;
    this.parameters = parameters;
    this.schema = schema;
  }
}

class ToolResult {
  constructor(success, data = null, error = null, metadata = {}) {
    this.success = success;
    this.data = data;
    this.error = error;
    this.metadata = metadata;
  }

  static success(data, metadata = {}) {
    return new ToolResult(true, data, null, { timestamp: Date.now(), ...metadata });
  }

  static error(message, metadata = {}) {
    return new ToolResult(false, null, message, { timestamp: Date.now(), ...metadata });
  }

  static fromResult(result) {
    return result.success 
      ? ToolResult.success(result.data, result.metadata)
      : ToolResult.error(result.error, result.metadata);
  }
}

const ToolCategory = Object.freeze({
  FILE: 'file',
  SHELL: 'shell',
  SEARCH: 'search',
  NETWORK: 'network',
  CUSTOM: 'custom',
});

class BaseTool {
  constructor(metadata) {
    this.metadata = {
      name: '',
      description: '',
      category: ToolCategory.CUSTOM,
      dangerous: false,
      parameters: {},
      schema: {},
      ...metadata,
    };
  }

  async execute(args) {
    throw new Error('execute() must be implemented by subclass');
  }

  validate(args) {
    return args != null;
  }

  success(data, metadata = {}) {
    return ToolResult.success(data, { toolName: this.metadata.name, ...metadata });
  }

  failure(error, metadata = {}) {
    return ToolResult.error(error, { toolName: this.metadata.name, ...metadata });
  }

  toToolDef() {
    return new ToolDef(
      this.metadata.name,
      this.metadata.description,
      (args) => this.execute(args),
      this.metadata.parameters,
      this.metadata.schema
    );
  }

  static fromFunction(fn, name, description, schema = {}) {
    return new FunctionTool(fn, name, description, schema);
  }
}

class FunctionTool extends BaseTool {
  constructor(fn, name, description, schema = {}) {
    super({ name, description, schema });
    this._fn = fn;
  }

  async execute(args) {
    try {
      const result = await this._fn(args);
      return this.success(result);
    } catch (error) {
      return this.failure(error.message || String(error));
    }
  }
}

class ToolRegistry {
  constructor(tools = []) {
    this._tools = new Map();
    for (const tool of tools) {
      this.add(tool);
    }
  }

  add(tool) {
    if (tool instanceof BaseTool) {
      this._tools.set(tool.metadata.name, tool);
    } else if (tool instanceof ToolDef) {
      this._tools.set(tool.name, tool);
    } else if (typeof tool === 'function') {
      const t = BaseTool.fromFunction(tool, tool.name || 'anonymous', tool.description || '');
      this._tools.set(t.metadata.name, t);
    } else {
      throw new Error('Tool must be BaseTool, ToolDef, or function');
    }
    return this;
  }

  register(tool) {
    return this.add(tool);
  }

  remove(name) {
    this._tools.delete(name);
    return this;
  }

  unregister(name) {
    return this.remove(name);
  }

  get(name) {
    return this._tools.get(name);
  }

  has(name) {
    return this._tools.has(name);
  }

  list() {
    return Array.from(this._tools.keys());
  }

  listTools() {
    return Array.from(this._tools.values());
  }

  listToolDefs() {
    const results = [];
    for (const tool of this._tools.values()) {
      if (tool instanceof BaseTool) {
        results.push(tool.toToolDef());
      } else {
        results.push(tool);
      }
    }
    return results;
  }

  listByCategory(category) {
    return this.listTools().filter(t => t instanceof BaseTool && t.metadata.category === category);
  }

  filter(predicate) {
    const filtered = this.listTools().filter(predicate);
    return new ToolRegistry(filtered);
  }

  merge(other) {
    for (const tool of other.listTools()) {
      this.add(tool);
    }
    return this;
  }

  size() {
    return this._tools.size;
  }

  clear() {
    this._tools.clear();
    return this;
  }

  toJSON() {
    return this.list().map(name => {
      const tool = this._tools.get(name);
      return tool instanceof BaseTool ? tool.metadata : tool;
    });
  }
}

function tool(description, options = {}) {
  return function (target, propertyKey, descriptor) {
    const originalMethod = descriptor.value;
    const toolName = options.name || propertyKey;
    const schema = options.schema || {};
    const properties = schema.properties || {};

    const wrapper = function (args) {
      const params = {};
      for (const [key, spec] of Object.entries(properties)) {
        params[key] = args[key] !== undefined ? args[key] : spec.default;
      }
      return originalMethod(params);
    };

    const toolDef = new ToolDef(toolName, description, wrapper, properties, schema);
    descriptor.value.toolDef = toolDef;
    descriptor.value.toolName = toolName;
    return descriptor;
  };
}

function createToolClass(classDefinition) {
  for (const [name, method] of Object.entries(classDefinition.prototype)) {
    if (name === 'constructor' || typeof method !== 'function') continue;
    if (method.toolDef) {
      const td = method.toolDef;
      classDefinition.prototype[name] = function(...args) {
        return method.call(this, ...args);
      };
      classDefinition.prototype[name].toolDef = td;
      classDefinition.prototype[name].toolName = td.name;
    }
  }
  return classDefinition;
}

function decorateClass(targetClass) {
  const proto = targetClass.prototype;
  const className = targetClass.name;
  
  for (const name of Object.getOwnPropertyNames(proto)) {
    const method = proto[name];
    if (name === 'constructor' || typeof method !== 'function') continue;
    if (method.toolDef) continue;
    
    const desc = String(method).match(/^\(?[^)]*\)?\s*=>|^function\s*\([^)]*\)/);
  }
  
  return targetClass;
}

function extractTools(instance) {
  const tools = [];
  const proto = Object.getPrototypeOf(instance);
  for (const name of Object.getOwnPropertyNames(proto)) {
    const method = instance[name];
    if (name === 'constructor' || typeof method !== 'function') continue;
    if (method.toolDef) {
      tools.push(method.toolDef);
    }
  }
  return tools;
}

function toolMethod(description, options = {}) {
  return function (target, propertyKey, descriptor) {
    return tool(description, options)(target, propertyKey, descriptor);
  };
}

class SessionManager {
  constructor(inner) {
    this._inner = inner;
  }

  async save(path) {
    await this._inner.saveMessageLog(path);
    return this;
  }

  async restore(path) {
    await this._inner.restoreMessageLog(path);
    return this;
  }

  async saveFull(path) {
    await this._inner.saveSession(path);
    return this;
  }

  async restoreFull(path) {
    await this._inner.restoreSessionFromFile(path);
    return this;
  }

  compact(keepRecent = 10, maxSummaryChars = 2000) {
    this._inner.compactSession(keepRecent, maxSummaryChars);
    return this;
  }

  clear() {
    this._inner.clearSession();
    return this;
  }

  getMessages() {
    return this._inner.getMessages();
  }

  addMessage(role, content) {
    this._inner.addMessage({ role, content });
    return this;
  }

  export() {
    return this._inner.getSessionJson();
  }

  import(json) {
    this._inner.restoreSessionJson(json);
    return this;
  }
}

class AgentBuilder {
  constructor(bus, options = {}) {
    this._bus = bus;
    this._inner = null;
    this._tools = new ToolRegistry();
    this._hooks = [];
    this._plugins = [];
    this._skills = [];
    this._mcpServers = [];
    this._config = {
      name: options.name || 'assistant',
      model: options.model || DEFAULT_MODEL,
      baseUrl: options.baseUrl || DEFAULT_BASE_URL,
      apiKey: options.apiKey,
      systemPrompt: options.systemPrompt || 'You are a helpful assistant.',
      temperature: options.temperature ?? 0.7,
      timeoutSecs: options.timeoutSecs || 120,
      maxTokens: options.maxTokens,
    };
  }

  name(name) {
    this._config.name = name;
    return this;
  }

  withConfig(config) {
    if (config.name) this._config.name = config.name;
    if (config.model) this._config.model = config.model;
    if (config.baseUrl) this._config.baseUrl = config.baseUrl;
    if (config.apiKey) this._config.apiKey = config.apiKey;
    if (config.systemPrompt) this._config.systemPrompt = config.systemPrompt;
    if (config.temperature !== undefined) this._config.temperature = config.temperature;
    if (config.timeoutSecs) this._config.timeoutSecs = config.timeoutSecs;
    if (config.maxTokens) this._config.maxTokens = config.maxTokens;
    if (config.circuitBreaker) {
      this._config.circuitBreakerMaxFailures = config.circuitBreaker.maxFailures;
      this._config.circuitBreakerCooldownSecs = config.circuitBreaker.cooldownSecs;
    }
    if (config.rateLimit) {
      this._config.rateLimitCapacity = config.rateLimit.capacity;
      this._config.rateLimitWindowSecs = config.rateLimit.windowSecs;
      this._config.rateLimitMaxRetries = config.rateLimit.maxRetries;
    }
    return this;
  }

  model(model) {
    this._config.model = model;
    return this;
  }

  baseUrl(url) {
    this._config.baseUrl = url;
    return this;
  }

  apiKey(key) {
    this._config.apiKey = key;
    return this;
  }

  system(prompt) {
    this._config.systemPrompt = prompt;
    return this;
  }

  prompt(prompt) {
    return this.system(prompt);
  }

  temperature(temp) {
    this._config.temperature = temp;
    return this;
  }

  timeout(secs) {
    this._config.timeoutSecs = secs;
    return this;
  }

  maxTokens(tokens) {
    this._config.maxTokens = tokens;
    return this;
  }

  tools(...tools) {
    for (const t of tools) {
      if (t instanceof ToolRegistry) {
        this._tools.merge(t);
      } else {
        this._tools.add(t);
      }
    }
    return this;
  }

  register(...tools) {
    return this.tools(...tools);
  }

  withTools(...tools) {
    return this.tools(...tools);
  }

  bash(name = 'bash', workspaceRoot = null) {
    this._config._bashTool = { name, workspaceRoot };
    return this;
  }

  circuitBreaker(maxFailures, cooldownSecs = 30) {
    this._config.circuitBreakerMaxFailures = maxFailures;
    this._config.circuitBreakerCooldownSecs = cooldownSecs;
    return this;
  }

  rateLimit(capacity, windowSecs = 60, maxRetries = 3) {
    this._config.rateLimitCapacity = capacity;
    this._config.rateLimitWindowSecs = windowSecs;
    this._config.rateLimitMaxRetries = maxRetries;
    return this;
  }

  resilience(config) {
    if (config.circuitBreaker) {
      this.circuitBreaker(config.circuitBreaker.maxFailures, config.circuitBreaker.cooldownSecs);
    }
    if (config.rateLimit) {
      this.rateLimit(config.rateLimit.capacity, config.rateLimit.windowSecs, config.rateLimit.maxRetries);
    }
    return this;
  }

  hook(event, callback) {
    this._hooks.push({ event, callback });
    return this;
  }

  hooks(hooks) {
    for (const [event, callback] of Object.entries(hooks)) {
      this._hooks.push({ event, callback });
    }
    return this;
  }

  plugin(name, handlers = {}) {
    this._plugins.push({ name, ...handlers });
    return this;
  }

  skill(name, content) {
    this._skills.push({ name, content });
    return this;
  }

  skillsFromDir(dirPath) {
    this._skills.push({ dirPath });
    return this;
  }

  mcp(namespace, command, args) {
    this._mcpServers.push({ namespace, command, args, type: 'process' });
    return this;
  }

  mcpHttp(namespace, url) {
    this._mcpServers.push({ namespace, url, type: 'http' });
    return this;
  }

  async start() {
    const { Agent: RawAgent } = require('./index.js');
    this._inner = await RawAgent.create(this._config);

    for (const td of this._tools.listToolDefs()) {
      await this._inner.addTool(td.name, td.description, JSON.stringify(td.parameters), JSON.stringify(td.schema), (err, args) => td.callback(args));
    }

    if (this._config._bashTool) {
      await this._inner.addBashTool(this._config._bashTool.name, this._config._bashTool.workspaceRoot);
    }

    for (const h of this._hooks) {
      this._inner.registerHook(h.event, h.callback);
    }

    for (const p of this._plugins) {
      this._inner.registerPlugin(p.name, p.onRequest, p.onResponse, p.onToolCall, p.onToolResult);
    }

    for (const s of this._skills) {
      if (s.dirPath) {
        await this._inner.registerSkillsFromDir(s.dirPath);
      }
    }

    for (const m of this._mcpServers) {
      if (m.type === 'process') {
        await this._inner.addMcpServer(m.namespace, m.command, m.args);
      } else {
        await this._inner.addMcpServerHttp(m.namespace, m.url);
      }
    }

    return new Agent(this._inner, this._tools);
  }

  async ask(prompt) {
    if (!this._inner) await this.start();
    return this._inner.runSimple(prompt);
  }

  async react(task) {
    if (!this._inner) await this.start();
    return this._inner.react(task);
  }

  stream(task, onToken) {
    if (!this._inner) throw new Error('Agent not started');
    return this._inner.stream(task, (err, token) => {
      if (err) {
        onToken({ type: 'Error', error: err.message || String(err) });
      } else {
        onToken(token);
      }
    });
  }

  async streamCollect(task) {
    const tokens = [];
    await new Promise((resolve, reject) => {
      this.stream(task, token => {
        tokens.push(token);
        if (token.type === 'Done' || token.type === 'Error') {
          token.type === 'Error' ? reject(new Error(token.error)) : resolve();
        }
      });
    });
    return tokens;
  }
}

class Agent {
  constructor(inner, tools) {
    this._inner = inner;
    this._tools = tools;
  }

  async ask(prompt) {
    return this._inner.runSimple(prompt);
  }

  async react(task) {
    return this._inner.react(task);
  }

  stream(task, onToken) {
    return this._inner.stream(task, (err, token) => {
      if (err) {
        onToken({ type: 'Error', error: err.message || String(err) });
      } else {
        onToken(token);
      }
    });
  }

  async streamCollect(task) {
    const tokens = [];
    await new Promise((resolve, reject) => {
      this.stream(task, token => {
        tokens.push(token);
        if (token.type === 'Done' || token.type === 'Error') {
          token.type === 'Error' ? reject(new Error(token.error)) : resolve();
        }
      });
    });
    return tokens;
  }

  get session() {
    return new SessionManager(this._inner);
  }

  get tools() {
    return this._inner.listTools();
  }

  get config() {
    return this._inner.config();
  }

  get inner() {
    return this._inner;
  }

  async listMcpTools() {
    return this._inner.listMcpTools();
  }

  get metrics() {
    return this._inner.getPerfMetrics();
  }

  resetMetrics() {
    this._inner.resetPerfMetrics();
  }
}

class BusManager {
  constructor(options = {}) {
    this._mode = options.mode || 'peer';
    this._connect = options.connect;
    this._listen = options.listen;
    this._peer = options.peer;
    this._bus = null;
  }

  static async create(options = {}) {
    const manager = new BusManager(options);
    return manager.start();
  }

  async start() {
    const { Bus: RawBus } = require('./index.js');
    this._bus = await RawBus.create({
      mode: this._mode,
      connect: this._connect,
      listen: this._listen,
      peer: this._peer,
    });
    return this;
  }

  async stop() {
    this._bus = null;
  }

  mode(mode) {
    this._mode = mode;
    return this;
  }

  connect(addresses) {
    this._connect = addresses;
    return this;
  }

  listen(addresses) {
    this._listen = addresses;
    return this;
  }

  peer(id) {
    this._peer = id;
    return this;
  }

  async publish(topic, payload, isJson = false) {
    if (!this._bus) throw new Error('Bus not started');
    return isJson 
      ? this._bus.publishJson(topic, payload)
      : this._bus.publishText(topic, payload);
  }

  async publisher(topic) {
    if (!this._bus) throw new Error('Bus not started');
    return new PublisherWrapper(await this._bus.createPublisher(topic));
  }

  async subscriber(topic) {
    if (!this._bus) throw new Error('Bus not started');
    return new SubscriberWrapper(await this._bus.createSubscriber(topic));
  }

  async query(topic) {
    if (!this._bus) throw new Error('Bus not started');
    return new QueryClient(await this._bus.createQuery(topic));
  }

  async queryable(topic, handler) {
    if (!this._bus) throw new Error('Bus not started');
    const q = await this._bus.createQueryable(topic);
    if (handler) q.setHandler(handler);
    return new QueryableServer(q);
  }

  async caller(name) {
    if (!this._bus) throw new Error('Bus not started');
    return new CallerClient(await this._bus.createCaller(name));
  }

  async callable(uri, handler) {
    if (!this._bus) throw new Error('Bus not started');
    const c = await this._bus.createCallable(uri);
    if (handler) c.setHandler(handler);
    return new CallableServer(c);
  }

  get bus() {
    if (!this._bus) throw new Error('Bus not started');
    return this._bus;
  }
}

class PublisherWrapper {
  constructor(inner) {
    this._inner = inner;
  }

  get topic() {
    return this._inner.topic;
  }

  async publish(payload, isJson = false) {
    return isJson 
      ? this._inner.publishJson(payload)
      : this._inner.publishText(payload);
  }

  async text(payload) {
    return this._inner.publishText(payload);
  }

  async json(data) {
    return this._inner.publishJson(data);
  }
}

class SubscriberWrapper {
  constructor(inner) {
    this._inner = inner;
  }

  get topic() {
    return this._inner.topic;
  }

  async recv(timeoutMs) {
    return timeoutMs
      ? this._inner.recvWithTimeoutMs(timeoutMs)
      : this._inner.recv();
  }

  async recvJson(timeoutMs) {
    return timeoutMs
      ? this._inner.recvJsonWithTimeoutMs(timeoutMs)
      : this._inner.recvJson();
  }

  async run(callback) {
    return this._inner.run(callback);
  }

  async runJson(callback) {
    return this._inner.runJson(callback);
  }

  [Symbol.asyncIterator]() {
    return this;
  }

  async next() {
    const msg = await this.recv();
    return msg === null ? { done: true } : { done: false, value: msg };
  }
}

class QueryClient {
  constructor(inner) {
    this._inner = inner;
  }

  get topic() {
    return this._inner.topic;
  }

  async ask(payload, timeoutMs) {
    return timeoutMs
      ? this._inner.queryTextTimeoutMs(payload, timeoutMs)
      : this._inner.queryText(payload);
  }

  async askJson(payload, timeoutMs) {
    const response = await this.ask(JSON.stringify(payload), timeoutMs);
    try {
      return JSON.parse(response);
    } catch {
      return response;
    }
  }
}

class QueryableServer {
  constructor(inner) {
    this._inner = inner;
  }

  handle(handler) {
    this._inner.setHandler(handler);
    return this;
  }

  async start() {
    await this._inner.start();
    return this;
  }

  async run(handler) {
    await this._inner.run(handler);
  }

  async runJson(handler) {
    await this._inner.runJson(handler);
  }
}

class CallerClient {
  constructor(inner) {
    this._inner = inner;
  }

  async call(payload) {
    return this._inner.callText(payload);
  }

  async callJson(payload) {
    return this._inner.callText(JSON.stringify(payload));
  }
}

class CallableServer {
  constructor(inner) {
    this._inner = inner;
  }

  handle(handler) {
    this._inner.setHandler(handler);
    return this;
  }

  get isStarted() {
    return this._inner.isStarted();
  }

  async start() {
    await this._inner.start();
    return this;
  }

  async run(handler) {
    await this._inner.run(handler);
  }

  async runJson(handler) {
    await this._inner.runJson(handler);
  }
}

class Config {
  constructor(options = {}) {
    this._loader = new ConfigLoader();
    this._loaded = false;
    this._config = null;
    this._options = options;
  }

  static load(options = {}) {
    return new Config(options).discover().load();
  }

  static fromFile(path) {
    const config = new Config();
    config._loader.addFile(path);
    return config.load();
  }

  static fromDirectory(path) {
    const config = new Config();
    config._loader.addDirectory(path);
    return config.load();
  }

  static fromInline(data) {
    const config = new Config();
    config._loader.addInline(data);
    return config.load();
  }

  file(path) {
    this._loader.addFile(path);
    return this;
  }

  directory(path) {
    this._loader.addDirectory(path);
    return this;
  }

  inline(data) {
    this._loader.addInline(data);
    return this;
  }

  discover() {
    this._loader.discover();
    return this;
  }

  reset() {
    this._loader.reset();
    this._loaded = false;
    this._config = null;
    return this;
  }

  load() {
    try {
      this._config = JSON.parse(this._loader.loadSync());
      this._loaded = true;
    } catch (e) {
      this._config = {};
      this._loaded = true;
    }
    return this;
  }

  reload() {
    this._loader.reloadSync();
    return this.load();
  }

  get(key, defaultValue = null) {
    if (!this._loaded) this.load();
    const keys = key.split('.');
    let value = this._config;
    for (const k of keys) {
      if (value && typeof value === 'object' && k in value) {
        value = value[k];
      } else {
        return defaultValue;
      }
    }
    return value;
  }

  get globalModel() {
    return this.get('global_model', {});
  }

  get model() {
    return this.get('global_model.model', 'nvidia/meta/llama-3.1-8b-instruct');
  }

  get baseUrl() {
    return this.get('global_model.base_url', 'https://integrate.api.nvidia.com/v1');
  }

  get apiKey() {
    return this.get('global_model.api_key', '');
  }

  get bus() {
    return this.get('bus', {});
  }

  toJSON() {
    return this._config || {};
  }

  isLoaded() {
    return this._loaded;
  }
}

class BrainOS {
  constructor(options = {}) {
    this._options = options;
    this._bus = null;
    this._registry = new ToolRegistry();
    this._started = false;
    this._config = null;
  }

  async start() {
    const config = Config.load();
    const gm = config.globalModel;

    this._apiKey = this._options.apiKey || gm.api_key;
    this._baseUrl = this._options.baseUrl || gm.base_url || DEFAULT_BASE_URL;
    this._model = this._options.model || gm.model || DEFAULT_MODEL;
    this._config = config;

    this._bus = await BusManager.create({ mode: 'peer' });
    this._started = true;
    return this;
  }

  async stop() {
    if (this._bus) {
      await this._bus.stop();
      this._bus = null;
    }
    this._started = false;
  }

  get isStarted() {
    return this._started;
  }

  agent(name = 'assistant', options = {}) {
    if (!this._started) {
      throw new Error('BrainOS not started. Call start() first.');
    }
    return new AgentBuilder(this._bus.bus, {
      name,
      model: options.model || this._model,
      baseUrl: options.baseUrl || this._baseUrl,
      apiKey: options.apiKey || this._apiKey,
      systemPrompt: options.systemPrompt || 'You are a helpful assistant.',
      temperature: options.temperature ?? 0.7,
      timeoutSecs: options.timeoutSecs || 120,
    }).tools(this._registry);
  }

  get bus() {
    if (!this._started) throw new Error('BrainOS not started');
    return this._bus;
  }

  get config() {
    return this._config;
  }

  get registry() {
    return this._registry;
  }

  get isStarted() {
    return this._started;
  }

  get registry() {
    return this._registry;
  }

  registerGlobal(...tools) {
    this._registry.add(...tools);
    return this;
  }

  tools(...tools) {
    return this.registerGlobal(...tools);
  }

  async createBus(options = {}) {
    return BusManager.create(options);
  }
}

module.exports = {
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
  Publisher: PublisherWrapper,
  Subscriber: SubscriberWrapper,
  Query: QueryClient,
  Queryable: QueryableServer,
  Caller: CallerClient,
  Callable: CallableServer,
  SessionManager,
  McpClient,
  Config,
  ConfigLoader,
  extractTools,
  version: getVersion,
  enableTracing,
  logTestMessage,
  HookEvent,
  HookDecision,
  HookContextData,
  HookRegistry,
};
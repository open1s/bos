/**
 * brainos-js — Elegant JavaScript API for BrainOS Agent Framework
 * 
 * A high-level wrapper around jsbos that provides:
 * - Async context manager for lifecycle management
 * - @tool() decorator for simple tool registration
 * - Fluent agent creation with chainable config
 * - Minimal boilerplate
 * 
 * Usage:
 *   import { BrainOS, tool } from './brainos.js';
 * 
 *   const brain = new BrainOS();
 *   await brain.start();
 *   const agent = brain.agent('assistant');
 *   const result = await agent.ask('What is 42 + 58?');
 */

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
  initTracing,
  logTestMessage,
} = require('./index.js');

class ToolDef {
  constructor(name, description, callback, parameters = {}, schema = {}) {
    this.name = name;
    this.description = description;
    this.callback = callback;
    this.parameters = parameters;
    this.schema = schema;
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
    
    const toolDef = new ToolDef(
      toolName,
      description,
      wrapper,
      properties,
      schema
    );
    
    descriptor.value.toolDef = toolDef;
    descriptor.value.toolName = toolName;
    
    return descriptor;
  };
}

class Agent {
  constructor(bus, options = {}) {
    this._bus = bus;
    this._inner = null;
    this._tools = [];
    this._config = {
      name: options.name || 'assistant',
      model: options.model || 'nvidia/meta/llama-3.1-8b-instruct',
      baseUrl: options.baseUrl || 'https://integrate.api.nvidia.com/v1',
      apiKey: options.apiKey,
      systemPrompt: options.systemPrompt || 'You are a helpful assistant.',
      temperature: options.temperature ?? 0.7,
      timeoutSecs: options.timeoutSecs || 120,
    };
  }

  withModel(model) {
    this._config.model = model;
    return this;
  }

  withPrompt(prompt) {
    this._config.systemPrompt = prompt;
    return this;
  }

  withTemperature(temp) {
    this._config.temperature = temp;
    return this;
  }

  withTimeout(secs) {
    this._config.timeoutSecs = secs;
    return this;
  }

  register(toolDef) {
    this._tools.push(toolDef);
    return this;
  }

  registerMany(...tools) {
    for (const t of tools) {
      this._tools.push(t);
    }
    return this;
  }

  async start() {
    const { Agent: RawAgent } = require('./jsbos.js');
    this._inner = await RawAgent.create(this._config);
    for (const t of this._tools) {
      const schema = JSON.stringify(t.schema);
      const params = JSON.stringify(t.parameters);
      await this._inner.addTool(
        t.name,
        t.description,
        params,
        schema,
        (err, args) => t.callback(args)
      );
    }
    return this;
  }

  async ask(question) {
    if (!this._inner) await this.start();
    return this._inner.runSimple(question);
  }

  async chat(message) {
    return this.ask(message);
  }

  async runSimple(message) {
    if (!this._inner) await this.start();
    return this._inner.runSimple(message);
  }

  async react(task) {
    if (!this._inner) await this.start();
    return this._inner.react(task);
  }

  get tools() {
    if (!this._inner) return this._tools.map(t => t.name);
    return this._inner.listTools();
  }

  get config() {
    return { ...this._config };
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
    const { Bus: RawBus } = require('./jsbos.js');
    const cfg = {
      mode: this._mode,
      connect: this._connect,
      listen: this._listen,
      peer: this._peer,
    };
    this._bus = await RawBus.create(cfg);
    return this;
  }

  async stop() {
    this._bus = null;
  }

  async publishText(topic, payload) {
    if (!this._bus) throw new Error('Bus not started');
    await this._bus.publishText(topic, payload);
  }

  async publishJson(topic, data) {
    if (!this._bus) throw new Error('Bus not started');
    await this._bus.publishJson(topic, data);
  }

  async createPublisher(topic) {
    if (!this._bus) throw new Error('Bus not started');
    const raw = await this._bus.createPublisher(topic);
    return new PublisherWrapper(raw);
  }

  async createSubscriber(topic) {
    if (!this._bus) throw new Error('Bus not started');
    const raw = await this._bus.createSubscriber(topic);
    return new SubscriberWrapper(raw);
  }

  async createQuery(topic) {
    if (!this._bus) throw new Error('Bus not started');
    const raw = await this._bus.createQuery(topic);
    return new QueryClient(raw);
  }

  async createQueryable(topic, handler = null) {
    if (!this._bus) throw new Error('Bus not started');
    const raw = await this._bus.createQueryable(topic);
    if (handler) raw.setHandler(handler);
    return new QueryableServer(raw);
  }

  async createCaller(name) {
    if (!this._bus) throw new Error('Bus not started');
    const raw = await this._bus.createCaller(name);
    return new CallerClient(raw);
  }

  async createCallable(uri, handler = null) {
    if (!this._bus) throw new Error('Bus not started');
    const raw = await this._bus.createCallable(uri);
    if (handler) raw.setHandler(handler);
    return new CallableServer(raw);
  }

  get bus() {
    if (!this._bus) throw new Error('Bus not started. Call start() first.');
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

  async publishText(payload) {
    await this._inner.publishText(payload);
  }

  async publishJson(data) {
    await this._inner.publishJson(data);
  }
}

class SubscriberWrapper {
  constructor(inner) {
    this._inner = inner;
  }

  get topic() {
    return this._inner.topic;
  }

  async recv() {
    return this._inner.recv();
  }

  async recvWithTimeoutMs(timeoutMs) {
    return this._inner.recvWithTimeoutMs(timeoutMs);
  }

  async recvJsonWithTimeoutMs(timeoutMs) {
    return this._inner.recvJsonWithTimeoutMs(timeoutMs);
  }

  async run(callback) {
    await this._inner.run(callback);
  }

  async runJson(callback) {
    await this._inner.runJson(callback);
  }

  [Symbol.asyncIterator]() {
    return this;
  }

  async next() {
    const msg = await this.recv();
    if (msg === null) return { done: true };
    return { done: false, value: msg };
  }
}

class QueryClient {
  constructor(inner) {
    this._inner = inner;
  }

  get topic() {
    return this._inner.topic;
  }

  async queryText(payload) {
    return this._inner.queryText(payload);
  }

  async queryTextTimeoutMs(payload, timeoutMs) {
    return this._inner.queryTextTimeoutMs(payload, timeoutMs);
  }
}

class QueryableServer {
  constructor(inner) {
    this._inner = inner;
  }

  setHandler(handler) {
    this._inner.setHandler(handler);
  }

  async start() {
    await this._inner.start();
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

  async callText(payload) {
    return this._inner.callText(payload);
  }
}

class CallableServer {
  constructor(inner) {
    this._inner = inner;
  }

  setHandler(handler) {
    this._inner.setHandler(handler);
  }

  get isStarted() {
    return this._inner.isStarted();
  }

  async start() {
    await this._inner.start();
  }

  async run(handler) {
    await this._inner.run(handler);
  }

  async runJson(handler) {
    await this._inner.runJson(handler);
  }
}

class BrainOS {
  constructor(options = {}) {
    this._options = options;
    this._bus = null;
  }

  async start() {
    const loader = new ConfigLoader();
    loader.discover();
    const config = JSON.parse(loader.loadSync());

    const globalModel = config.global_model || {};
    this._apiKey = this._options.apiKey || globalModel.api_key;
    this._baseUrl = this._options.baseUrl || globalModel.base_url || 'https://integrate.api.nvidia.com/v1';
    this._model = this._options.model || globalModel.model || 'nvidia/meta/llama-3.1-8b-instruct';

    this._bus = await Bus.create();
    return this;
  }

  async stop() {
    this._bus = null;
  }

  agent(name = 'assistant', options = {}) {
    if (!this._bus) {
      throw new Error('BrainOS not started. Call start() first.');
    }
    return new Agent(this._bus, {
      name,
      model: options.model || this._model,
      baseUrl: options.baseUrl || this._baseUrl,
      apiKey: options.apiKey || this._apiKey,
      systemPrompt: options.systemPrompt || 'You are a helpful assistant.',
      temperature: options.temperature ?? 0.7,
      timeoutSecs: options.timeoutSecs || 120,
    });
  }

  get bus() {
    if (!this._bus) {
      throw new Error('BrainOS not started. Call start() first.');
    }
    return this._bus;
  }
}

module.exports = {
  BrainOS,
  Agent,
  tool,
  ToolDef,
  BusManager,
  Publisher: PublisherWrapper,
  Subscriber: SubscriberWrapper,
  Query: QueryClient,
  Queryable: QueryableServer,
  Caller: CallerClient,
  Callable: CallableServer,
  McpClient,
  version: getVersion,
  initTracing,
  logTestMessage,
};
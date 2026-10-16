/* Low-level native bindings (NAPI-RS auto-generated) */
export declare class ExternalObject<T> {
  readonly '': {
    readonly '': unique symbol
    [K: symbol]: T
  }
}
/** A JavaScript-facing agent. */
export declare class Agent {
  /** Create an agent from `config`. */
  static create(config: AgentConfig): Promise<Agent>
  /** Create an agent from `config`, using `bus`. */
  static createWithBus(config: AgentConfig, bus: ExternalObject<Session>): Promise<Agent>
  /** Run a single task and return the answer. */
  runSimple(task: string | Array<JsContent>): Promise<string>
  /** Run the ReAct loop for `task`. */
  react(task: string | Array<JsContent>): Promise<string>
  /** Return the effective agent configuration. */
  config(): any
  /** List registered tool names. */
  listTools(): Array<string>
  /** List registered async tool names. */
  listAsyncTools(): Array<string>
  /** Register `hook` for `event`. */
  registerHook(event: HookEvent, callback: ((err: Error | null, arg: HookContextData) => unknown)): void
  /** Register `plugin` with the agent. */
  registerPlugin(name: string, onLlmRequest?: (((err: Error | null, arg: JSAny) => unknown)) | undefined | null, onLlmResponse?: (((err: Error | null, arg: JSAny) => unknown)) | undefined | null, onToolCall?: (((err: Error | null, arg: JSAny) => unknown)) | undefined | null, onToolResult?: (((err: Error | null, arg: JSAny) => unknown)) | undefined | null): void
  /** Close the agent and release resources. */
  close(): void
  /** Stop the running agent. */
  stop(options?: StopOptions | undefined | null): any
  /** Whether the agent is running. */
  isRunning(): boolean
  /** Register a tool implemented in JavaScript. */
  addTool(name: string, description: string, parameters: string, schema: string, callback: ((err: Error | null, arg: JSAny) => unknown), cancelable: boolean, cancelCallback?: (((err: Error | null, arg: string) => unknown)) | undefined | null): Promise<string>
  /** Register a sandboxed bash tool. */
  addBashTool(name: string, workspaceRoot?: string | undefined | null): Promise<void>
  /** Register skills discovered in `dir_path`. */
  registerSkillsFromDir(dirPath: string): Promise<void>
  /** Add a stdio MCP server. */
  addMcpServer(namespace: string, command: string, args: Array<string>): Promise<void>
  /** Add an HTTP MCP server under `namespace`. */
  addMcpServerHttp(namespace: string, url: string): Promise<void>
  /** List tools across all MCP servers. */
  listMcpTools(): Promise<Array<any>>
  /** List resources for an MCP namespace. */
  listMcpResources(namespace: string): Promise<Array<any>>
  /** List prompts across all MCP servers. */
  listMcpPrompts(): Promise<Array<any>>
  /** Register a tool that calls a remote agent endpoint over the bus. */
  addRemoteAgentTool(toolName: string, endpoint: string): Promise<void>
  /** Build an RPC client for this agent. */
  rpcClient(endpoint: string, bus: ExternalObject<Session>): Promise<AgentRpcClient>
  /** Serve this agent as a callable server. */
  asCallableServer(endpoint: string, bus: ExternalObject<Session>): Promise<AgentCallableServer>
  /** Run `task` and stream tokens to `callback`. */
  stream(task: string | Array<JsContent>, callback: ((err: Error | null, arg: any) => unknown)): Promise<string>
  /** Return the session history as JSON. */
  getSessionJson(): string
  /** Export the session as JSON. */
  exportSession(): string
  /** Restore the session from JSON. */
  restoreSessionJson(json: string): void
  /** Save the session to `path`. */
  saveSession(path: string): void
  /** Restore the session from `path`. */
  restoreSessionFromFile(path: string): void
  /** Clear the session history. */
  clearSession(): void
  /** Return the opaque session context value. */
  sessionContext(): any
  /** Replace the opaque session context value. */
  setSessionContext(context: any): void
  /** Reset the opaque session context to null. */
  clearSessionContext(): void
  /** Compact the session, keeping recent turns. */
  compactSession(keepRecent: number, maxSummaryChars: number): void
  /** Persist the agent's conversation to a file path. */
  saveMessageLog(path: string): void
  /** Load a previously saved conversation from a file path. */
  restoreMessageLog(path: string): void
  /** The conversation messages, serialized as JSON. */
  getMessages(): any
  /** Append a role/content message to the conversation. */
  addMessage(message: any): void
  /** Return a snapshot of performance metrics. */
  getPerfMetrics(): PerfSnapshot
  /** Reset all performance metrics. */
  resetPerfMetrics(): void
}

/** Serves calls for a JavaScript agent. */
export declare class AgentCallableServer {
  /** The server endpoint. */
  get endpoint(): string
  /** Whether the server has started. */
  isStarted(): boolean
}

/** Client for calling a remote agent. */
export declare class AgentRpcClient {
  /** The RPC endpoint. */
  get endpoint(): string
  /** List the remote tools. */
  list(): Promise<any>
  /** Call a remote tool. */
  call(toolName: string, argsJson: string): Promise<any>
}

/** A handle to the message bus. */
export declare class Bus {
  /** Create a bus from an optional configuration. */
  static create(config?: BusConfig | undefined | null): Promise<Bus>
  /** Return a handle to the bus session. */
  session(): Promise<ExternalObject<Session>>
  /** Return the session identifier. */
  sessionId(): string
  /** Publish a text payload to `topic`. */
  publishText(topic: string, payload: string): Promise<void>
  /** Publish a JSON payload to `topic`. */
  publishJson(topic: string, data: any): Promise<void>
  /** Create a publisher bound to the bus session. */
  createPublisher(topic: string): Promise<Publisher>
  /** Create a subscriber bound to the bus session. */
  createSubscriber(topic: string): Promise<Subscriber>
  /** Create a query bound to the bus session. */
  createQuery(topic: string): Promise<Query>
  /** Create a queryable bound to the bus session. */
  createQueryable(topic: string): Promise<Queryable>
  /** Create a caller bound to the bus session. */
  createCaller(name: string): Promise<Caller>
  /** Create a callable bound to the bus session. */
  createCallable(uri: string): Promise<Callable>
  /**
   * Close the bus, aborting all running tasks and releasing resources.
   * Closes the underlying Zenoh session, allowing the process to exit cleanly.
   */
  close(): Promise<void>
}

/** Serves callable requests over the bus. */
export declare class Callable {
  /** Set the handler invoked for each call. */
  setHandler(handler: ((err: Error | null, arg: string) => unknown)): void
  /** Whether the callable has started. */
  isStarted(): boolean
  /** Start serving with the configured handler. */
  start(): Promise<void>
  /** Set `handler` and start serving. */
  run(handler: ((err: Error | null, arg: string) => unknown)): Promise<void>
  /** Set `handler` and start serving JSON. */
  runJson(handler: ((err: Error | null, arg: string) => unknown)): Promise<void>
  /** Stop the callable, aborting the handler task and dropping the subscription. */
  stop(): Promise<void>
}

/** Issues request/response calls over the bus. */
export declare class Caller {
  /** Create a caller without a session. */
  static new(name: string): Promise<Caller>
  /** Create a caller bound to `session`. */
  static withSession(name: string, session: ExternalObject<Session>): Promise<Caller>
  /** Call the handler and return its text result. */
  callText(payload: string): Promise<string>
}

/** Loads layered configuration. */
export declare class ConfigLoader {
  /** Create a loader with default discovery. */
  constructor()
  /** Discover configuration files. */
  discover(): void
  /** Add a configuration file. */
  addFile(path: string): void
  /** Add a configuration directory. */
  addDirectory(path: string): void
  /** Merge an inline JSON object. */
  addInline(data: any): void
  /** Reset the loader to an empty state. */
  reset(): void
  /** Load and return the merged config as JSON. */
  loadSync(): string
  /** Reload sources and return the merged config as JSON. */
  reloadSync(): string
}

/** Registry of agent hooks. */
export declare class HookRegistry {
  /** Create an empty registry. */
  constructor()
  /** Register `callback` for `event`. */
  register(event: HookEvent, callback: ((err: Error | null, arg: HookContextData) => unknown)): Promise<void>
}

/** A client for a Model Context Protocol server. */
export declare class McpClient {
  /** Spawn a stdio MCP server. */
  static spawn(command: string, args: Array<string>): Promise<McpClient>
  /** Create a client for an HTTP MCP endpoint. */
  static connectHttp(url: string): McpClient
  /** Initialize the session and return server capabilities. */
  initialize(): Promise<any>
  /** List the tools the server exposes. */
  listTools(): Promise<Array<any>>
  /** Call a tool by name. */
  callTool(name: string, argsJson: string): Promise<any>
  /** List the prompts the server exposes. */
  listPrompts(): Promise<Array<any>>
  /** List the resources the server exposes. */
  listResources(): Promise<Array<any>>
  /** Read a resource by URI. */
  readResource(uri: string): Promise<any>
}

/** Registry of agent plugins. */
export declare class PluginRegistry {
  /** Create an empty registry. */
  constructor()
  /** Remove all registered plugins. */
  clear(): void
}

export declare class PluginToolCallInfo {
  id: string
  name: string
  arguments: string
}

/** Publishes messages to a topic. */
export declare class Publisher {
  /** Create a publisher for `topic`. */
  static new(topic: string): Promise<Publisher>
  /** Create a publisher bound to `session`. */
  static withSession(topic: string, session: ExternalObject<Session>): Promise<Publisher>
  /** The published topic. */
  get topic(): string
  /** Publish a text payload. */
  publishText(payload: string): Promise<void>
  /** Publish a JSON payload. */
  publishJson(data: any): Promise<void>
}

/** Runs queries against a queryable. */
export declare class Query {
  /** Create a query for `topic`. */
  static new(topic: string): Promise<Query>
  /** Create a query bound to `session`. */
  static withSession(topic: string, session: ExternalObject<Session>): Promise<Query>
  /** The queried topic. */
  get topic(): string
  /** Query the handler and return its text result. */
  queryText(payload: string): Promise<string>
  /** Query the handler, waiting up to `timeout_ms`. */
  queryTextTimeoutMs(payload: string, timeoutMs: number): Promise<string>
}

/** Serves queries over the bus. */
export declare class Queryable {
  /** Create a queryable for `topic`. */
  static new(topic: string): Promise<Queryable>
  /** Set the handler invoked for each query. */
  setHandler(handler: ((err: Error | null, arg: string) => unknown)): void
  /** Start serving with the configured handler. */
  start(): Promise<void>
  /** Set `handler` and start serving. */
  run(handler: ((err: Error | null, arg: string) => unknown)): Promise<void>
  /** Set `handler` and start serving JSON. */
  runJson(handler: ((err: Error | null, arg: string) => unknown)): Promise<void>
  /** Stop the queryable, aborting the handler task and dropping the subscription. */
  stop(): Promise<void>
  /** Set `handler` and start serving streaming results. */
  runStream(handler: ((err: Error | null, arg: string) => unknown)): Promise<void>
}

/** Subscribes to a topic. */
export declare class Subscriber {
  /** Create a subscriber for `topic`. */
  static new(topic: string): Promise<Subscriber>
  /** Create a subscriber bound to `session`. */
  static withSession(topic: string, session: ExternalObject<Session>): Promise<Subscriber>
  /** The subscribed topic. */
  get topic(): string
  /** Receive the next message. */
  recv(): Promise<string | null>
  /** Receive the next message, waiting up to `timeout_ms`. */
  recvWithTimeoutMs(timeoutMs: number): Promise<string | null>
  /** Receive the next message parsed as JSON. */
  recvJsonWithTimeoutMs(timeoutMs: number): Promise<any | null>
  /** Deliver messages to `handler` until stopped. */
  run(handler: ((err: Error | null, arg: JSAny) => any)): Promise<void>
  /** Deliver messages parsed as JSON to `handler`. */
  runJson(handler: ((err: Error | null, arg: JSAny) => any)): Promise<void>
  /** Stop the delivery loop. */
  stop(): Promise<void>
}

/** Configuration for an agent. */
export interface AgentConfig {
  /** Agent name. */
  name: string
  /** Model identifier. */
  model: string
  /** LLM API base URL. */
  baseUrl: string
  /** LLM API key. */
  apiKey: string
  /** System prompt. */
  systemPrompt: string
  /** Sampling temperature. */
  temperature: number
  /** Completion token cap. */
  maxTokens?: number
  /** Per-request timeout in seconds. */
  timeoutSecs: number
  /** Maximum ReAct steps. */
  maxSteps?: number
  /** API protocol. */
  apiMode?: string
  /** Reasoning effort, if any. */
  reasoningEffort?: string
  /** Circuit breaker failure threshold. */
  circuitBreakerMaxFailures?: number
  /** Circuit breaker cooldown in seconds. */
  circuitBreakerCooldownSecs?: number
  /** Rate limiter burst capacity. */
  rateLimitCapacity?: number
  /** Rate limiter window in seconds. */
  rateLimitWindowSecs?: number
  /** Maximum retries when rate limited. */
  rateLimitMaxRetries?: number
  /** Retry backoff in seconds. */
  rateLimitRetryBackoffSecs?: number
  /** Whether to wait automatically when rate limited. */
  rateLimitAutoWait?: boolean
}

/** Status of a token budget. */
export declare const enum BudgetStatus {
  /** Within budget. */
  Normal = 0,
  /** Nearing the budget. */
  Warning = 1,
  /** Over the budget. */
  Exceeded = 2,
  /** Critically over the budget. */
  Critical = 3
}

/** JavaScript-facing bus configuration. */
export interface BusConfig {
  /** Transport mode. */
  mode: string
  /** Endpoints to connect to. */
  connect?: Array<string>
  /** Endpoints to listen on. */
  listen?: Array<string>
  /** Peer endpoint for peer mode. */
  peer?: string
}

/** Context passed to a hook. */
export interface HookContextData {
  /** Agent the hook is running for. */
  agentId: string
  /** Arbitrary string data. */
  data: Record<string, string>
}

/** A hook verdict. */
export declare const enum HookDecision {
  /** Continue the run. */
  Continue = 0,
  /** Abort the run. */
  Abort = 1,
  /** Fail the run. */
  Error = 2
}

/** Agent lifecycle events hooks can observe. */
export declare const enum HookEvent {
  /** Before a tool call. */
  BeforeToolCall = 0,
  /** After a tool call. */
  AfterToolCall = 1,
  /** Before an LLM call. */
  BeforeLlmCall = 2,
  /** After an LLM call. */
  AfterLlmCall = 3,
  /** When a message is added. */
  OnMessage = 4,
  /** When a run completes. */
  OnComplete = 5,
  /** When an error occurs. */
  OnError = 6
}

/** Initialize tracing for the process. */
export declare function initTracing(): void

export interface JsContent {
  type: string
  text?: string
  contentType?: string
  url?: string
  base64?: string
  name?: string
}

/** Token usage reported by an LLM provider. */
export interface LlmUsage {
  /** Prompt tokens. */
  promptTokens: number
  /** Completion tokens. */
  completionTokens: number
  /** Total tokens. */
  totalTokens: number
  /** Optional prompt token breakdown. */
  promptTokensDetails?: PromptTokensDetails
}

/** Emit a test log message. */
export declare function logTestMessage(message: string): void

/**
 * Performance metrics collected across LLM calls.
 * All timing values are in microseconds.
 */
export interface PerfSnapshot {
  /** Number of LLM API calls completed */
  llmCallCount: number
  /** Total wall-clock time across calls. */
  totalWallTimeUs: number
  /** Average wall-clock time per call. */
  avgWallTimeUs: number
  /** Minimum wall-clock time. */
  minWallTimeUs: number
  /** Maximum wall-clock time. */
  maxWallTimeUs: number
  /** Total engine time. */
  totalEngineTimeUs: number
  /** Total time spent in resilience layers. */
  totalResilienceTimeUs: number
  /** Number of rate-limit waits. */
  rateLimitWaits: number
  /** Total time spent waiting on rate limits. */
  totalRateLimitWaitUs: number
  /** Number of circuit breaker trips. */
  circuitTrips: number
  /** Number of LLM errors. */
  llmErrors: number
  /** Number of tool invocations (not LLM calls) */
  toolInvocationCount: number
  /** Total tool execution time. */
  totalToolTimeUs: number
  /** Total input tokens. */
  totalInputTokens: number
  /** Total output tokens. */
  totalOutputTokens: number
}

/** View of an outgoing LLM request. */
export interface PluginLlmRequest {
  /** Prompt input. */
  input: string
  /** Model identifier. */
  model: string
  /** Sampling temperature. */
  temperature?: number
  /** Completion token cap. */
  maxTokens?: number
  /** Nucleus sampling probability. */
  topP?: number
  /** Top-k sampling cutoff. */
  topK?: number
  /** Reasoning effort, if any. */
  reasoningEffort?: string
  /** API protocol. */
  apiMode: string
  /** Arbitrary metadata. */
  metadata: Record<string, string>
}

/** An LLM response seen by a plugin. */
export type PluginLlmResponse =
  | { type: 'OpenAI', /** Response id. */
  id: string, /** Model identifier. */
  model: string, /** Text content, if any. */
  content?: string, /** Response kind. */
responseType?: string }
| { type: 'Responses', /** Response id. */
id: string, /** Model identifier. */
model: string, /** Text content, if any. */
content?: string, /** Response kind. */
responseType?: string }

/** The stage at which a plugin runs. */
export declare const enum PluginStage {
  /** Before a request is sent. */
  PreRequest = 0,
  /** After a response is received. */
  PostResponse = 1,
  /** Before a tool executes. */
  PreExecute = 2,
  /** After a tool executes. */
  PostExecute = 3
}

/** View of a tool call. */
export interface PluginToolCall {
  /** Tool name. */
  name: string
  /** JSON-encoded arguments. */
  args: string
  /** Provider call id. */
  id?: string
  /** Arbitrary metadata. */
  metadata: Record<string, string>
}

/** View of a tool result. */
export interface PluginToolResult {
  /** Serialized tool result. */
  result: string
  /** Whether the call succeeded. */
  success: boolean
  /** Error message, if the call failed. */
  error?: string
  /** Arbitrary metadata. */
  metadata: Record<string, string>
}

/** Token details for the prompt portion of a call. */
export interface PromptTokensDetails {
  /** Audio tokens in the prompt. */
  audioTokens?: number
  /** Cached tokens in the prompt. */
  cachedTokens?: number
}

export interface StopOptions {
  clearSession?: boolean
}

/** A token budget report. */
export interface TokenBudgetReport {
  /** Token usage included in the report. */
  usage: TokenUsage
  /** Budget status. */
  status: BudgetStatus
  /** Fraction of the budget used. */
  usagePercent: number
  /** Tokens left in the budget. */
  remainingTokens: number
}

/** Token usage for a single agent run. */
export interface TokenUsage {
  /** Prompt tokens. */
  promptTokens: number
  /** Completion tokens. */
  completionTokens: number
  /** Total tokens. */
  totalTokens: number
}

/** Return the binding version. */
export declare function version(): string

/* High-level API types (tsc auto-generated from index.js) */
export * from "./jsbos.js";
import * as jsbos from './jsbos.js';
export class BrainOS {
    static create(options?: {}): Promise<BrainOS>;
    constructor(options?: {});
    _options: {};
    _bus: BusManager | null;
    _registry: ToolRegistry;
    _started: boolean;
    _config: Config | null;
    start(): Promise<this>;
    _apiKey: any;
    _baseUrl: any;
    _model: any;
    stop(): Promise<void>;
    get isStarted(): boolean;
    agent(name?: string, options?: {}): AgentBuilder;
    get bus(): BusManager | null;
    get config(): Config | null;
    get registry(): ToolRegistry;
    registerGlobal(...tools: any[]): this;
    tools(...tools: any[]): this;
    createBus(options?: {}): Promise<BusManager>;
}
export class AgentBuilder {
    constructor(bus: any, options?: {});
    _bus: any;
    _inner: jsbos.Agent | null;
    _tools: ToolRegistry;
    _hooks: any[];
    _plugins: any[];
    _skills: any[];
    _mcpServers: any[];
    _memory: any;
    _recallLimit: any;
    _config: {
        name: any;
        model: any;
        baseUrl: any;
        apiKey: any;
        systemPrompt: any;
        temperature: any;
        timeoutSecs: any;
        maxTokens: any;
        apiMode: any;
        reasoningEffort: any;
    };
    name(name: any): this;
    withConfig(config: any): this;
    model(model: any): this;
    baseUrl(url: any): this;
    apiKey(key: any): this;
    system(prompt: any): this;
    prompt(prompt: any): this;
    temperature(temp: any): this;
    timeout(secs: any): this;
    maxTokens(tokens: any): this;
    apiMode(mode: any): this;
    reasoningEffort(effort: any): this;
    withMemory(memory: any, limit?: number): this;
    tools(...tools: any[]): this;
    register(...tools: any[]): this;
    withTools(...tools: any[]): this;
    bash(name?: string, workspaceRoot?: null): this;
    circuitBreaker(maxFailures: any, cooldownSecs?: number): this;
    rateLimit(capacity: any, windowSecs?: number, maxRetries?: number): this;
    resilience(config: any): this;
    hook(event: any, callback: any): this;
    hooks(hooks: any): this;
    plugin(nameOrObj: any, handlers?: {}): this;
    plugins(...plugins: any[]): this;
    skill(name: any, content: any): this;
    skillsDir(dirPath: any): this;
    mcp(namespace: any, command: any, args: any): this;
    mcpHttp(namespace: any, url: any): this;
    start(): Promise<this>;
    _contentPartsToJsContentArray(input: any): any;
    _resolveContent(input: any): any;
    _withMemory(prompt: any, content: any): any;
    ask(prompt: any): Promise<string>;
    chat(message: any): Promise<string>;
    runSimple(prompt: any): Promise<string>;
    react(task: any): Promise<string>;
    stream(task: any, onToken: any): Promise<string>;
    streamCollect(task: any): Promise<any[]>;
    stop(options?: {}): any;
    isRunning(): boolean;
    get session(): SessionManager;
    get config(): any;
    get toolNames(): any[];
    get metrics(): jsbos.PerfSnapshot | null;
    resetMetrics(): this;
    listMcpTools(): Promise<any[]>;
    listMcpResources(namespace: any): Promise<any[]>;
    listMcpPrompts(): Promise<any[]>;
}
export function tool(descriptionOrOptions: any, maybeOptions?: {}): (target: any, propertyKey: any, descriptor: any) => any;
export class ToolDef {
    constructor(name: any, description: any, callback: any, parameters?: {}, schema?: {}, cancelCallback?: null);
    name: any;
    description: any;
    callback: any;
    parameters: {};
    schema: {};
    cancelCallback: any;
}
export class ToolResult {
    static success(data: any, metadata?: {}): ToolResult;
    static error(message: any, metadata?: {}): ToolResult;
    static fromResult(result: any): ToolResult;
    constructor(success: any, data?: null, error?: null, metadata?: {});
    success: any;
    data: any;
    error: any;
    metadata: {};
}
export const ToolCategory: Readonly<{
    FILE: "file";
    SHELL: "shell";
    SEARCH: "search";
    NETWORK: "network";
    CUSTOM: "custom";
}>;
export class BaseTool {
    static fromFunction(fn: any, name: any, description: any, schema?: {}): FunctionTool;
    constructor(metadata: any);
    metadata: any;
    execute(args: any): Promise<void>;
    validate(args: any): boolean;
    success(data: any, metadata?: {}): ToolResult;
    failure(error: any, metadata?: {}): ToolResult;
    toToolDef(): ToolDef;
}
export class FunctionTool extends BaseTool {
    constructor(fn: any, name: any, description: any, schema?: {});
    _fn: any;
    execute(args: any): Promise<ToolResult>;
}
export class ToolRegistry {
    constructor(tools?: any[]);
    _tools: Map<any, any>;
    add(tool: any): this;
    register(tool: any): this;
    remove(name: any): this;
    unregister(name: any): this;
    get(name: any): any;
    has(name: any): boolean;
    list(): any[];
    listTools(): any[];
    listToolDefs(): any[];
    listByCategory(category: any): any[];
    filter(predicate: any): ToolRegistry;
    merge(other: any): this;
    size(): number;
    clear(): this;
    toJSON(): any[];
}
export class BusManager {
    static create(options?: {}): Promise<BusManager>;
    constructor(options?: {});
    _mode: any;
    _connect: any;
    _listen: any;
    _peer: any;
    _bus: jsbos.Bus | null;
    start(): Promise<this>;
    stop(): Promise<void>;
    mode(mode: any): this;
    connect(addresses: any): this;
    listen(addresses: any): this;
    peer(id: any): this;
    publish(topic: any, payload: any, isJson?: boolean): Promise<void>;
    publisher(topic: any): Promise<PublisherWrapper>;
    subscriber(topic: any): Promise<SubscriberWrapper>;
    query(topic: any): Promise<QueryClient>;
    queryable(topic: any, handler: any): Promise<QueryableServer>;
    caller(name: any): Promise<CallerClient>;
    callable(uri: any, handler: any): Promise<CallableServer>;
    get bus(): jsbos.Bus;
    get sessionId(): string;
}
export class PublisherWrapper {
    constructor(inner: any);
    _inner: any;
    get topic(): any;
    publish(payload: any, isJson?: boolean): Promise<any>;
    /** @deprecated Use publishText. */
    text(payload: any): Promise<any>;
    /** @deprecated Use publishJson. */
    json(data: any): Promise<any>;
    publishText(payload: any): Promise<any>;
    publishJson(data: any): Promise<any>;
}
export class SubscriberWrapper {
    constructor(inner: any);
    _inner: any;
    get topic(): any;
    recv(timeoutMs: any): Promise<any>;
    recvJson(timeoutMs: any): Promise<any>;
    run(callback: any): Promise<any>;
    runJson(callback: any): Promise<any>;
    next(): Promise<{
        done: boolean;
        value?: undefined;
    } | {
        done: boolean;
        value: any;
    }>;
    [Symbol.asyncIterator](): this;
}
export class QueryClient {
    constructor(inner: any);
    _inner: any;
    get topic(): any;
    ask(payload: any, timeoutMs: any): Promise<any>;
    askJson(payload: any, timeoutMs: any): Promise<any>;
}
export class QueryableServer {
    constructor(inner: any);
    _inner: any;
    handle(handler: any): this;
    start(): Promise<this>;
    run(handler: any): Promise<void>;
    runJson(handler: any): Promise<void>;
}
export class CallerClient {
    constructor(inner: any);
    _inner: any;
    call(payload: any): Promise<any>;
    callJson(payload: any): Promise<any>;
}
export class CallableServer {
    constructor(inner: any);
    _inner: any;
    handle(handler: any): this;
    get isStarted(): any;
    start(): Promise<this>;
    run(handler: any): Promise<void>;
    runJson(handler: any): Promise<void>;
}
export class SessionManager {
    constructor(inner: any);
    _inner: any;
    save(path: any): Promise<this>;
    restore(path: any): Promise<this>;
    saveFull(path: any): Promise<this>;
    restoreFull(path: any): Promise<this>;
    compact(keepRecent?: number, maxSummaryChars?: number): this;
    clear(): this;
    getMessages(): any;
    addMessage(role: any, content: any): this;
    export(): any;
    exportJson(): any;
    importSession(data: any): this;
    import(data: any): this;
    get context(): any;
    setContext(context: any): this;
    clearContext(): this;
}
export class Config {
    static load(options?: {}): Config;
    static fromFile(path: any): Config;
    static fromDirectory(path: any): Config;
    static fromInline(data: any): Config;
    constructor(options?: {});
    _loader: jsbos.ConfigLoader;
    _loaded: boolean;
    _config: any;
    _options: {};
    file(path: any): this;
    directory(path: any): this;
    inline(data: any): this;
    discover(): this;
    reset(): this;
    load(): this;
    reload(): this;
    get(key: any, defaultValue?: null): any;
    get globalModel(): any;
    get model(): any;
    get baseUrl(): any;
    get apiKey(): any;
    get bus(): any;
    toJSON(): any;
    isLoaded(): boolean;
}
export function extractTools(instance: any): ToolDef[];
export function defineTool(name: any, description: any): (paramSchema: any) => {
    (callback: any): ToolDef;
    returns(returnSchema: any): (callback: any) => ToolDef;
};
export function defineTools(toolDefs: any): {};
export function createTool(name: any, description: any): (paramSchema: any) => {
    (callback: any): ToolDef;
    returns(returnSchema: any): (callback: any) => ToolDef;
};
export class Binary {
    static fromBase64(contentType: any, data: any, name?: null): Binary;
    static fromUrl(contentType: any, url: any, name?: null): Binary;
    constructor(contentType: any, source: any, name?: null);
    contentType: any;
    source: any;
    name: any;
    isImage(): any;
    isAudio(): any;
    url(): any;
    toJSON(): {
        content_type: any;
        source: {
            url: any;
            base64: any;
        };
    };
}
export class Content {
    static text(text: any): Content;
    static parts(parts: any): Content;
    static image(url: any, name?: null): Content;
    static audio(data: any, format?: string): Content;
    static audioUrl(url: any, format?: string): Content;
    constructor(parts?: null, text?: null);
    _parts: any;
    _text: any;
    toJSON(): any;
    toString(): string;
    isMultimodal(): boolean;
}
export class ContentPart {
    static text(text: any): ContentPart;
    static binary(contentType: any, data: any, name?: null): ContentPart;
    static binaryUrl(contentType: any, url: any, name?: null): ContentPart;
    static image(url: any, name?: null): ContentPart;
    static audio(data: any, format?: string): ContentPart;
    static audioUrl(url: any, format?: string): ContentPart;
    constructor(type: any, data?: {});
    type: any;
    text: any;
    binary: any;
    toJSON(): {
        type: any;
    };
}
export class Memory {
    static load(path: any): Memory;
    constructor(maxItems?: null);
    _items: any[];
    _maxItems: any;
    withMaxItems(maxItems: any): this;
    get maxItems(): any;
    add(content: any, metadata?: null): {
        id: any;
        content: string;
        metadata: null;
        created_at_ms: number;
    };
    all(): any[];
    search(query: any, limit?: number, metadata?: null): any[];
    remove(id: any): boolean;
    clear(): void;
    save(path: any): number;
    len(): number;
    isEmpty(): boolean;
    recallBlock(query: any, limit?: number): string;
    toJSON(): any[];
}
export { jsbos };
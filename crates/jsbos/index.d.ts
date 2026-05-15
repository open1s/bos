/* Low-level native bindings (NAPI-RS auto-generated) */
export declare class ExternalObject<T> {
  readonly '': {
    readonly '': unique symbol
    [K: symbol]: T
  }
}
export declare class Agent {
  static create(config: AgentConfig): Promise<Agent>
  static createWithBus(config: AgentConfig, bus: ExternalObject<Session>): Promise<Agent>
  runSimple(task: string): Promise<string>
  react(task: string): Promise<string>
  config(): any
  listTools(): Array<string>
  registerHook(event: HookEvent, callback: ((err: Error | null, arg: HookContextData) => any)): void
  registerPlugin(name: string, onLlmRequest?: (((err: Error | null, arg: JSAny) => any)) | undefined | null, onLlmResponse?: (((err: Error | null, arg: JSAny) => any)) | undefined | null, onToolCall?: (((err: Error | null, arg: JSAny) => any)) | undefined | null, onToolResult?: (((err: Error | null, arg: JSAny) => any)) | undefined | null): void
  close(): void
  addTool(name: string, description: string, parameters: string, schema: string, callback: ((err: Error | null, arg: JSAny) => any)): Promise<string>
  addBashTool(name: string, workspaceRoot?: string | undefined | null): Promise<void>
  registerSkillsFromDir(dirPath: string): Promise<void>
  addMcpServer(namespace: string, command: string, args: Array<string>): Promise<void>
  addMcpServerHttp(namespace: string, url: string): Promise<void>
  listMcpTools(): Promise<Array<any>>
  listMcpResources(namespace: string): Promise<Array<any>>
  listMcpPrompts(): Promise<Array<any>>
  rpcClient(endpoint: string, bus: ExternalObject<Session>): Promise<AgentRpcClient>
  asCallableServer(endpoint: string, bus: ExternalObject<Session>): Promise<AgentCallableServer>
  stream(task: string, callback: ((err: Error | null, arg: any) => any)): Promise<void>
  getSessionJson(): string
  exportSession(): string
  restoreSessionJson(json: string): void
  saveSession(path: string): void
  restoreSessionFromFile(path: string): void
  clearSession(): void
  compactSession(keepRecent: number, maxSummaryChars: number): void
  getPerfMetrics(): PerfSnapshot
  resetPerfMetrics(): void
}

export declare class AgentCallableServer {
  get endpoint(): string
  isStarted(): boolean
}

export declare class AgentRpcClient {
  get endpoint(): string
  list(): Promise<any>
  call(toolName: string, argsJson: string): Promise<any>
}

export declare class Bus {
  static create(config?: BusConfig | undefined | null): Promise<Bus>
  session(): Promise<ExternalObject<Session>>
  sessionId(): string
  publishText(topic: string, payload: string): Promise<void>
  publishJson(topic: string, data: any): Promise<void>
  createPublisher(topic: string): Promise<Publisher>
  createSubscriber(topic: string): Promise<Subscriber>
  createQuery(topic: string): Promise<Query>
  createQueryable(topic: string): Promise<Queryable>
  createCaller(name: string): Promise<Caller>
  createCallable(uri: string): Promise<Callable>
}

export declare class Callable {
  setHandler(handler: ((err: Error | null, arg: string) => unknown)): void
  isStarted(): boolean
  start(): Promise<void>
  run(handler: ((err: Error | null, arg: string) => unknown)): Promise<void>
  runJson(handler: ((err: Error | null, arg: string) => unknown)): Promise<void>
}

export declare class Caller {
  static new(name: string): Promise<Caller>
  static withSession(name: string, session: ExternalObject<Session>): Promise<Caller>
  callText(payload: string): Promise<string>
}

export declare class ConfigLoader {
  constructor()
  discover(): void
  addFile(path: string): void
  addDirectory(path: string): void
  addInline(data: any): void
  reset(): void
  loadSync(): string
  reloadSync(): string
}

export declare class HookRegistry {
  constructor()
  register(event: HookEvent, callback: ((err: Error | null, arg: HookContextData) => any)): Promise<void>
}

export declare class McpClient {
  static spawn(command: string, args: Array<string>): Promise<McpClient>
  static connectHttp(url: string): McpClient
  initialize(): Promise<any>
  listTools(): Promise<Array<any>>
  callTool(name: string, argsJson: string): Promise<any>
  listPrompts(): Promise<Array<any>>
  listResources(): Promise<Array<any>>
  readResource(uri: string): Promise<any>
}

export declare class PluginRegistry {
  constructor()
  clear(): void
}

export declare class PluginToolCallInfo {
  id: string
  name: string
  arguments: string
}

export declare class Publisher {
  static new(topic: string): Promise<Publisher>
  static withSession(topic: string, session: ExternalObject<Session>): Promise<Publisher>
  get topic(): string
  publishText(payload: string): Promise<void>
  publishJson(data: any): Promise<void>
}

export declare class Query {
  static new(topic: string): Promise<Query>
  static withSession(topic: string, session: ExternalObject<Session>): Promise<Query>
  get topic(): string
  queryText(payload: string): Promise<string>
  queryTextTimeoutMs(payload: string, timeoutMs: number): Promise<string>
}

export declare class Queryable {
  static new(topic: string): Promise<Queryable>
  setHandler(handler: ((err: Error | null, arg: string) => unknown)): void
  start(): Promise<void>
  run(handler: ((err: Error | null, arg: string) => unknown)): Promise<void>
  runJson(handler: ((err: Error | null, arg: string) => unknown)): Promise<void>
  runStream(handler: ((err: Error | null, arg: string) => unknown)): Promise<void>
}

export declare class Subscriber {
  static new(topic: string): Promise<Subscriber>
  static withSession(topic: string, session: ExternalObject<Session>): Promise<Subscriber>
  get topic(): string
  recv(): Promise<string | null>
  recvWithTimeoutMs(timeoutMs: number): Promise<string | null>
  recvJsonWithTimeoutMs(timeoutMs: number): Promise<any | null>
  run(handler: ((err: Error | null, arg: JSAny) => any)): Promise<void>
  runJson(handler: ((err: Error | null, arg: JSAny) => any)): Promise<void>
  stop(): Promise<void>
}

export interface AgentConfig {
  name: string
  model: string
  baseUrl: string
  apiKey: string
  systemPrompt: string
  temperature: number
  maxTokens?: number
  timeoutSecs: number
  maxSteps?: number
  circuitBreakerMaxFailures?: number
  circuitBreakerCooldownSecs?: number
  rateLimitCapacity?: number
  rateLimitWindowSecs?: number
  rateLimitMaxRetries?: number
  rateLimitRetryBackoffSecs?: number
  rateLimitAutoWait?: boolean
  contextCompactionThresholdTokens?: number
  contextCompactionTriggerRatio?: number
  contextCompactionKeepRecentMessages?: number
  contextCompactionMaxSummaryChars?: number
  contextCompactionSummaryMaxTokens?: number
}

export declare const enum BudgetStatus {
  Normal = 0,
  Warning = 1,
  Exceeded = 2,
  Critical = 3
}

export interface BusConfig {
  mode: string
  connect?: Array<string>
  listen?: Array<string>
  peer?: string
}

export interface HookContextData {
  agentId: string
  data: Record<string, string>
}

export declare const enum HookDecision {
  Continue = 0,
  Abort = 1,
  Error = 2
}

export declare const enum HookEvent {
  BeforeToolCall = 0,
  AfterToolCall = 1,
  BeforeLlmCall = 2,
  AfterLlmCall = 3,
  OnMessage = 4,
  OnComplete = 5,
  OnError = 6
}

export declare function initTracing(): void

export interface LlmUsage {
  promptTokens: number
  completionTokens: number
  totalTokens: number
  promptTokensDetails?: PromptTokensDetails
}

export declare function logTestMessage(message: string): void

/**
 * Performance metrics collected across LLM calls.
 * All timing values are in microseconds.
 */
export interface PerfSnapshot {
  /** Number of LLM API calls completed */
  llmCallCount: number
  totalWallTimeUs: number
  avgWallTimeUs: number
  minWallTimeUs: number
  maxWallTimeUs: number
  totalEngineTimeUs: number
  totalResilienceTimeUs: number
  rateLimitWaits: number
  totalRateLimitWaitUs: number
  circuitTrips: number
  llmErrors: number
  /** Number of tool invocations (not LLM calls) */
  toolInvocationCount: number
  totalToolTimeUs: number
  totalInputTokens: number
  totalOutputTokens: number
}

export interface PluginLlmRequest {
  input: string
  model: string
  temperature?: number
  maxTokens?: number
  topP?: number
  topK?: number
  metadata: Record<string, string>
}

export type PluginLlmResponse =
  | { type: 'OpenAI', id: string, model: string, content?: string }

export declare const enum PluginStage {
  PreRequest = 0,
  PostResponse = 1,
  PreExecute = 2,
  PostExecute = 3
}

export interface PluginToolCall {
  name: string
  args: string
  id?: string
  metadata: Record<string, string>
}

export interface PluginToolResult {
  result: string
  success: boolean
  error?: string
  metadata: Record<string, string>
}

export interface PromptTokensDetails {
  audioTokens?: number
  cachedTokens?: number
}

export interface TokenBudgetReport {
  usage: TokenUsage
  status: BudgetStatus
  usagePercent: number
  remainingTokens: number
}

export interface TokenUsage {
  promptTokens: number
  completionTokens: number
  totalTokens: number
}

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
    _config: {
        name: any;
        model: any;
        baseUrl: any;
        apiKey: any;
        systemPrompt: any;
        temperature: any;
        timeoutSecs: any;
        maxTokens: any;
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
    skill(name: any, content: any): this;
    skillsFromDir(dirPath: any): this;
    mcp(namespace: any, command: any, args: any): this;
    mcpHttp(namespace: any, url: any): this;
    start(): Promise<jsbos.Agent>;
    ask(prompt: any): Promise<string>;
    react(task: any): Promise<string>;
    stream(task: any, onToken: any): Promise<void>;
    streamCollect(task: any): Promise<any[]>;
}
export function tool(descriptionOrOptions: any, maybeOptions?: {}): (target: any, propertyKey: any, descriptor: any) => any;
export class ToolDef {
    constructor(name: any, description: any, callback: any, parameters?: {}, schema?: {});
    name: any;
    description: any;
    callback: any;
    parameters: {};
    schema: {};
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
}
export class PublisherWrapper {
    constructor(inner: any);
    _inner: any;
    get topic(): any;
    publish(payload: any, isJson?: boolean): Promise<any>;
    text(payload: any): Promise<any>;
    json(data: any): Promise<any>;
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
    import(json: any): this;
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
export { jsbos };
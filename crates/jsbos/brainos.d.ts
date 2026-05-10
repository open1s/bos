/**
 * Type declarations for brainos.js high-level API
 */

export interface BusOptions {
  mode?: string;
  connect?: string[];
  listen?: string[];
  peer?: string;
}

export interface BrainOSOptions {
  apiKey?: string;
  baseUrl?: string;
  model?: string;
  systemPrompt?: string;
  temperature?: number;
  timeoutSecs?: number;
  maxTokens?: number;
  busMode?: string;
  busConnect?: string[];
  busListen?: string[];
  busPeer?: string;
}

export class BrainOS {
  constructor(options?: BrainOSOptions);
  start(): Promise<this>;
  stop(): Promise<void>;
  isStarted: boolean;
  agent(name?: string, options?: BrainOSOptions): AgentBuilder;
  bus: BusManager;
  config: Config;
  registry: ToolRegistry;
  registerGlobal(...tools: any[]): this;
  tools(...tools: any[]): this;
  createBus(options?: BusOptions): Promise<BusManager>;
  static create(options?: BrainOSOptions): Promise<BrainOS>;
}

export class Agent {
  constructor(bus: unknown, options?: object);
  withModel(model: string): Agent;
  withPrompt(prompt: string): Agent;
  withPrompt(prompt: string): Agent;
  withTemperature(temp: number): Agent;
  withTimeout(secs: number): Agent;
  withTools(...tools: ToolDef[]): Agent;
  withBashTool(name?: string, workspaceRoot?: string | null): Agent;
  register(toolDef: ToolDef): Agent;
  registerMany(...tools: ToolDef[]): Agent;
  onHook(event: number, callback: Function): Agent;
  start(): Promise<this>;
  ask(question: string): Promise<string>;
  chat(message: string): Promise<string>;
  runSimple(message: string): Promise<string>;
  react(task: string): Promise<string>;
  stream(task: string, onToken: (token: unknown) => void): void;
  streamCollect(task: string): Promise<unknown[]>;
  get tools(): string[];
  get config(): object;
  get _inner(): unknown;
}

export class ToolDef {
  constructor(
    name: string,
    description: string,
    callback: (args: unknown) => unknown,
    parameters?: object,
    schema?: object
  );
  name: string;
  description: string;
  parameters: object;
  schema: object;
  callback: (args: unknown) => unknown;
}

export function tool(description: string, options?: { schema?: object; name?: string }): MethodDecorator;
export function tool(options: { description?: string; schema?: object; name?: string }): MethodDecorator;

export type ParamSchema = Record<string, { type?: string; description?: string; default?: any; required?: boolean } | number>;
export type ReturnSchema = Record<string, { type?: string; description?: string } | number>;

export interface ToolDefOptions {
  description: string;
  params?: ParamSchema;
  returns?: ReturnSchema;
  fn?: (args: any) => any;
}

export function defineTool(name: string, description: string): {
  (params: ParamSchema): {
    (callback: (args: any) => any): ToolDef;
    returns(returnSchema: ReturnSchema): (callback: (args: any) => any) => ToolDef;
  };
};

export function defineTools(toolDefs: Record<string, ToolDefOptions>): Record<string, ToolDef>;

export const createTool: typeof defineTool;

export function extractTools(instance: object): ToolDef[];

export class BusManager {
  constructor(options?: object);
  static create(options?: object): Promise<BusManager>;
  start(): Promise<this>;
  stop(): Promise<void>;
  async publishText(topic: string, payload: string): Promise<void>;
  async publishJson(topic: string, data: object): Promise<void>;
  async createPublisher(topic: string): Promise<unknown>;
  async createSubscriber(topic: string): Promise<unknown>;
  async createQuery(topic: string): Promise<unknown>;
  async createQueryable(topic: string, handler?: Function): Promise<unknown>;
  async createCaller(name: string): Promise<unknown>;
  async createCallable(uri: string, handler?: Function): Promise<unknown>;
}

export class McpClient {
  static spawn(command: string, args: string[]): Promise<McpClient>;
  static connectHttp(url: string): McpClient;
  initialize(): Promise<any>;
  listTools(): Promise<any[]>;
  callTool(name: string, argsJson: string): Promise<any>;
  listPrompts(): Promise<any[]>;
  listResources(): Promise<any[]>;
  readResource(uri: string): Promise<any>;
}

export const HookEvent: {
  BeforeToolCall: number;
  AfterToolCall: number;
  BeforeLlmCall: number;
  AfterLlmCall: number;
  OnMessage: number;
  OnComplete: number;
  OnError: number;
};

export const HookDecision: {
  Continue: number;
  Abort: number;
  Error: number;
};

export function version(): string;
export function initTracing(): void;
export function logTestMessage(message: string): void;
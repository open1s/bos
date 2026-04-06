/**
 * TypeScript declarations for ESM wrapper of Agent
 */

export interface AgentConfig {
  name: string;
  model: string;
  base_url: string;
  api_key: string;
  system_prompt: string;
  temperature: number;
  max_tokens?: number;
  timeout_secs: number;
  max_steps?: number;
  context_compaction_threshold_tokens?: number;
  context_compaction_trigger_ratio?: number;
  context_compaction_keep_recent_messages?: number;
  context_compaction_max_summary_chars?: number;
  context_compaction_summary_max_tokens?: number;
}

export interface Agent {
  runSimple(task: string): Promise<string>;
  react(task: string): Promise<string>;
  config(): Promise<Record<string, unknown>>;
  listTools(): Promise<string[]>;
  addTool(
    name: string,
    description: string,
    parameters: string,
    schema: string,
    callback: (args: unknown) => unknown
  ): Promise<string>;
  registerSkillsFromDir(dirPath: string): Promise<void>;
  addMcpServer(namespace: string, command: string, args: string[]): Promise<void>;
  addMcpServerHttp(namespace: string, url: string): Promise<void>;
  listMcpTools(): Promise<Array<{ name: string; description: string }>>;
  listMcpResources(namespace: string): Promise<Array<{ name: string; description: string }>>;
  listMcpPrompts(): Promise<Array<{ name: string; description: string }>>;
  rpcClient(endpoint: string, bus: unknown): Promise<AgentRpcClient>;
  asCallableServer(endpoint: string, bus: unknown): Promise<AgentCallableServer>;
}

export interface AgentRpcClient {
  readonly endpoint: string;
  list(): Promise<unknown>;
  call(toolName: string, argsJson: string): Promise<unknown>;
  llmRun(task: string): Promise<unknown>;
}

export interface AgentCallableServer {
  readonly endpoint: string;
  readonly isStarted: boolean;
}

export type AgentStatic = {
  create(config: AgentConfig): Promise<Agent>;
  createWithBus(config: AgentConfig, bus: unknown): Promise<Agent>;
};

declare const Agent: AgentStatic;
declare const AgentRpcClient: new () => AgentRpcClient;
declare const AgentCallableServer: new () => AgentCallableServer;

export { Agent, AgentRpcClient, AgentCallableServer };

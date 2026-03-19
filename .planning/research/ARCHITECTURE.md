# Architecture Research

**Domain:** Distributed LLM Agent Orchestration Framework (Rust/Zenoh)
**Researched:** 2026-03-19
**Confidence:** MEDIUM-HIGH

## Component Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        Agent Application                         │
│                                                                  │
│  ┌──────────────────────────────────────────────────────────┐    │
│  │                    brainos Crate                          │    │
│  │                                                           │    │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐      │    │
│  │  │   Agent     │  │   Tool      │  │   Skill     │      │    │
│  │  │   Core      │  │   System    │  │   System    │      │    │
│  │  │             │  │             │  │             │      │    │
│  │  │ - LlmClient │  │ - Tool trait│  │ - Skill     │      │    │
│  │  │ - AgentLoop  │  │ - Registry │  │   Registry  │      │    │
│  │  │ - MessageLog │  │ - Schema   │  │ - Template  │      │    │
│  │  │ - Session    │  │   Trans.   │  │   Engine    │      │    │
│  │  └──────┬──────┘  └──────┬─────┘  └──────┬─────┘      │    │
│  │         │                │                │             │    │
│  │  ┌──────┴────────────────┴────────────────┴──────┐     │    │
│  │  │              MCP Bridge                          │     │    │
│  │  │  - MCP Client (STDIO)  →  Bus RPC Proxy         │     │    │
│  │  └──────────────────────┬─────────────────────────┘     │    │
│  │                         │                               │    │
│  │  ┌──────────────────────┴─────────────────────────┐     │    │
│  │  │              A2A Protocol                       │     │    │
│  │  │  - TaskState machine  - Agent Discovery         │     │    │
│  │  │  - Message envelope    on the bus               │     │    │
│  │  └──────────────────────┬─────────────────────────┘     │    │
│  │                         │                               │    │
│  │  ┌──────────────────────┴─────────────────────────┐     │    │
│  │  │              Scheduler                          │     │    │
│  │  │  - Sequential | Parallel | Conditional           │     │    │
│  │  │  - Timeout | Retry | Error handling             │     │    │
│  │  └─────────────────────────────────────────────────┘     │    │
│  └──────────────────────────┬──────────────────────────────┘    │
│                             │                                    │
│  ┌──────────────────────────┴──────────────────────────────┐    │
│  │                   bus Crate                             │    │
│  │  QueryableWrapper ← Agent (as a Queryable)               │    │
│  │  RpcClient ← Tool calls, A2A messages                    │    │
│  │  PublisherWrapper ← Agent broadcasts, streaming           │    │
│  │  rkyv serialization (zero-copy on bus)                 │    │
│  └──────────────────────────────────────────────────────────┘    │
│                                                                  │
│  ┌──────────────────────────────────────────────────────────┐    │
│  │                config Crate                              │    │
│  │  ConfigLoader → Agent definitions, Skills, Tools          │    │
│  └──────────────────────────────────────────────────────────┘    │
│                                                                  │
│  ┌──────────────────────────────────────────────────────────┐    │
│  │                     Zenoh Network                         │    │
│  │  (Discovery, Pub/Sub, Query — handled by zenoh)           │    │
│  └──────────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────────┘
```

## Component Boundaries

### Agent Core (`agent_core`)

**Responsibility:** The agent reasoning loop — given input, produce output.

**Public API:**
- `Agent::new(config, llm_client, tool_registry, skill_loader)`
- `Agent::run(&self, task: &str) -> AgentResult`
- `Agent::stream_run(&self, task: &str) -> impl Stream<Item = Token>`
- `AgentState::serialize()` / `AgentState::deserialize()`

**Key types:**
- `AgentConfig` — name, system prompt, model, tools, skills
- `MessageLog` — conversation history (user, assistant, tool result)
- `LlmClient` trait — provider abstraction

**Talks to:** LlmClient (trait), ToolRegistry, SkillLoader, Bus (via QueryableWrapper)

**Does NOT talk to:** Scheduler directly (scheduler calls Agent), MCP servers (via bridge)

### Tool System (`tools`)

**Responsibility:** Define, register, and execute tools.

**Public API:**
- `Tool` trait — name, description, JSON schema, execute
- `ToolRegistry::register(tool)` / `ToolRegistry::get(name)`
- `ToolSchemaTranslator` — convert Rust types to provider-specific schemas (OpenAI vs Anthropic)

**Key types:**
- `ToolCall` — parsed tool call from LLM response
- `ToolResult` — execution result with error handling
- `ToolError` — typed errors (schema mismatch, execution failed, timeout)

**Talks to:** Agent Core, MCP Bridge

**Does NOT talk to:** LLM client directly

### MCP Bridge (`mcp`)

**Responsibility:** Bridge MCP STDIO servers to bus-native RPC.

**Public API:**
- `McpClient::connect(server_path)` → MCP client
- `McpBridge::new(client)` → wraps as bus RPC
- `McpBridge::discover_tools()` → returns tools as `Vec<Box<dyn Tool>>`

**Key types:**
- `McpProtocolHandler` — JSON-RPC 2.0 over STDIO
- `McpToolAdapter` — converts MCP tool format to brainos `Tool` trait

**Talks to:** Tool System, Bus

**Does NOT talk to:** LLM client directly

### A2A Protocol (`a2a`)

**Responsibility:** Agent-to-agent task delegation over the bus.

**Public API:**
- `A2aClient::new(agent_id)` — register on bus
- `A2aClient::delegate(task, target_agent)` → task_id
- `A2aClient::poll(task_id)` → TaskStatus
- `A2aServer` — handles incoming delegation requests

**Key types:**
- `A2aEnvelope` — `{method, params, task_id, reply_to, idempotency_key}`
- `TaskState` — `Submitted | Working | Completed(result) | Failed(err) | InputRequired`
- `AgentCapabilities` — what this agent can do (for discovery)

**Discovery:** Agents publish `AgentCapabilities` to `agents/capabilities/{agent_id}` on bus. Other agents subscribe to `agents/capabilities/*` to find who can do what.

**Talks to:** Bus (RpcClient, QueryableWrapper), Agent Core

**Does NOT talk to:** LLM client directly

### Scheduler (`scheduler`)

**Responsibility:** Orchestrate multi-step, multi-agent workflows.

**Public API:**
- `Workflow::new()` — build workflow
- `Workflow::add_step(agent_id, input)` — sequential step
- `Workflow::add_parallel(steps)` — parallel branch
- `Workflow::add_conditional(condition, then, else_)` — conditional
- `Executor::run(workflow)` → final result

**Key types:**
- `WorkflowStep` — agent_id + input + timeout
- `StepResult` — success/failure/timeout with output
- `WorkflowError` — step failed, all failed, timeout

**Talks to:** A2A Client, Agent Core

**Does NOT talk to:** LLM client, MCP

### Skill System (`skills`)

**Responsibility:** Load and compose agent capabilities from config.

**Public API:**
- `SkillLoader::from_config(config)` → SkillRegistry
- `SkillRegistry::attach_to(agent)`
- `SkillComposer::compose(skills)` → combined prompt fragment + tool references

**Key types:**
- `SkillDefinition` — loaded from TOML/YAML
- `SkillComposer` — merges skill prompts, detects conflicts

**Talks to:** Config (via ConfigLoader), Agent Core

**Does NOT talk to:** LLM client, Bus directly

### Session Manager (`session`)

**Responsibility:** Persist and restore agent state.

**Public API:**
- `SessionManager::save(agent_id, state)` → save to disk
- `SessionManager::load(agent_id)` → restore state
- `SessionManager::list()` → available sessions

**Key types:**
- `AgentState` — serializable, contains message log + context
- `SessionMetadata` — agent_id, created_at, last_updated

**Talks to:** Serialization (serde_json), File system

## Data Flow

### Agent Reasoning Loop
```
User Task
    ↓
AgentCore.run(task)
    ↓
MessageLog.append(user_message)
    ↓
LlmClient.complete(messages + tools + skills)
    ↓
Parse Response:
  ├─ Text response → MessageLog.append(assistant) → return
  └─ Tool call → ToolRegistry.execute(call)
                    ↓
                    ToolResult
                    ↓
                    MessageLog.append(tool_result)
                    ↓
                    LlmClient.complete(...) [loop]
```

### Tool Execution via RPC
```
ToolRegistry.execute(call)
    ↓
Bus RpcClient.call(tool_name, args)  [over Zenoh]
    ↓
Remote tool service responds
    ↓
ToolResult → Agent loop continues
```

### A2A Delegation
```
Agent A delegates task to Agent B
    ↓
A2aClient.delegate(task, "agent-b")
    ↓
Publish task to bus: `a2a/tasks/{task_id}`
    ↓
Agent B receives via QueryableWrapper
    ↓
Agent B processes, publishes result to `a2a/results/{task_id}`
    ↓
Agent A polls/polls for result
    ↓
TaskState → Completed(result)
```

### MCP → Bus Bridge
```
MCP server (STDIO)
    ↓
McpProtocolHandler.send_json_rpc("tools/list")
    ↓
McpToolAdapter converts MCP tools → brainos Tool trait
    ↓
Tools registered in ToolRegistry
    ↓
Tool execution → MCP protocol → STDIO → MCP server
```

## Suggested Build Order

**Phase 1 — Core Agent**
1. LLM Client trait + OpenAI-compatible implementation
2. Tool trait + registry
3. Agent core (reasoning loop)
4. Basic streaming
5. Config-driven agent loading

**Phase 2 — Integrations**
1. MCP bridge
2. A2A protocol
3. Skills system

**Phase 3 — Orchestration**
1. Scheduler
2. Session persistence
3. Error handling polish

**Why this order:** Each phase depends on the previous. Can't build A2A without agents. Can't build scheduler without A2A. But MCP, skills, and streaming can be parallelized within Phase 2.

## Key Architectural Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Agent as bus service | QueryableWrapper | Natural fit — agents receive tasks, return results |
| Tools as RPC calls | RpcClient | Zero new concepts, typed, timeout-able |
| MCP → bus bridge | Adapter pattern | MCP servers stay local, tools become distributed |
| A2A over bus | JSON-RPC envelope on bus | Reuse existing RPC infra, Zenoh handles transport |
| Skills as config | TOML/YAML loaded via ConfigLoader | Declarative, composable, no code changes |
| Streaming over bus | Token-by-token via PublisherWrapper | Simple, works with Zenoh pub/sub |

---
*Architecture research for: Distributed LLM Agent Orchestration Framework*
*Researched: 2026-03-19*

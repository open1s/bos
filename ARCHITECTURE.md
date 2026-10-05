# BrainOS Architecture

This document describes the overall architecture, design patterns, and system interactions in BrainOS (BOS).

## 🏛️ System Architecture Overview

BrainOS is structured as a modular, event-driven system with the following core layers:

```
┌─────────────────────────────────────────────────────────────┐
│                   Application Layer                         │
│              (User Applications & Services)                 │
└─────────────────────┬───────────────────────────────────────┘
                      │
┌─────────────────────┴───────────────────────────────────────┐
│                   ReAct Engine Layer                        │
│          (AI Agent Reasoning & Acting Orchestration)        │
└─────────────┬─────────────────────┬─────────────────────────┘
              │                     │
        ┌─────▼──────┐      ┌──────▼─────────┐
        │ Agent Crate│      │ Tools Registry │
        │   (Skills) │      │  (Extensible)  │
        └─────┬──────┘      └──────┬─────────┘
              │                     │
┌─────────────┴─────────────────────┴─────────────────────────┐
│                  Bus Layer (Event Pub/Sub)                  │
│         (Async Message Distribution & Streaming & RPC)      │
├─────────────┬─────────────────────┬─────────────────────────┤
│ Publisher   │   Queryable         │  Subscriber             │
│ (Emit)      │   (Request/Reply)   │  (Listen)               │
└─────────────┴─────────────────────┴─────────────────────────┘
              │                     │
┌─────────────┴─────────────────────┴────────────────────────┐
│              Infrastructure & Support Layer                │
├──────────────┬──────────────┬─────────────┬────────────────┤
│ Config       │ Logging      │ Memory      │ Resilience     │
│ (Settings)   │ (Telemetry)  │ (State)     │ (Circuit Break)│
└──────────────┴──────────────┴─────────────┴────────────────┘
              │                     │
┌─────────────┴─────────────────────┴─────────────────────────┐
│           External Systems & Integrations                    │
│  (LLMs, APIs, Databases, Message Queues, Zenoh)             │
└─────────────────────────────────────────────────────────────┘
```

---

## 🎯 Core Components

### 1. ReAct Engine (`crates/react`)

**Purpose**: Orchestrates the Reasoning + Acting loop for AI agents.

**Key Responsibilities**:
- Manages agent lifecycle and state
- Coordinates LLM calls and reasoning
- Routes actions to appropriate tools
- Handles timeouts and failures
- Persists memory across sessions

**Architecture** (actual tree):
```
crates/react/src
├── engine/              # ReAct loop + step orchestration
│   ├── mod.rs           # ReActEngine, react/react_stream, call-id helpers
│   ├── builder.rs       # ReActEngineBuilder
│   ├── llm_calls.rs     # call_llm / call_llm_stream / is_transient_error
│   ├── tool_calls.rs    # call_tool
│   ├── tool_run.rs      # ToolRunManager + ToolCallEvent
│   ├── skill_cache.rs   # SkillCache + CachedSkill
│   └── error.rs         # ReactError + BuilderError
├── llm/
│   ├── client.rs        # LlmClient trait (complete / stream_complete)
│   ├── response.rs      # LlmResponse, StreamToken, TokenStream
│   ├── types.rs         # LlmRequest, Content, LlmMessage, ReactSession/Context, LlmError
│   └── vendor/          # LLM provider adapters
│       ├── openaicompatible.rs  # shared OpenAI wire types, SSE extractor, tool-call accumulator
│       ├── openai.rs            # canonical OpenAI-compatible transport
│       ├── deepseek.rs | nvidia.rs | openrouter.rs  # thin delegating vendors
│       ├── responses.rs         # OpenAI Responses API transport
│       └── router.rs            # LlmRouter: named-vendor dispatch
├── tool/                # Tool / AsyncTool, ToolRegistry, descriptor, ToolError
├── resilience/          # circuit breaker, rate limiter, retry
├── runtime/             # ReActApp lifecycle-hook seam (NoopApp)
├── telemetry/           # token counting + budget
└── utils/               # streaming extractors (JSON / XML / mixed)
```

> **Two tool registries, by layer — not a duplicated seam.**
> `react::tool::ToolRegistry` is the engine-internal store (name → `ToolVariant`)
> used for dispatch and cancellation. `agent::tools::registry::ToolRegistry` is
> the user-facing registry (sync + async tools, JSON-schema cache, namespaces,
> MCP marking). They live in different layers and have different interfaces on
> purpose; `react::Tool` and `react::ToolError` are the shared seam both use.

**Data Flow**:
```
Input Prompt
    ↓
[Reasoning Phase]
    ├─ Call LLM with context
    ├─ Parse reasoning output
    ├─ Extract action & parameters
    ↓
[Acting Phase]
    ├─ Look up tool
    ├─ Execute tool (with timeout)
    ├─ Capture output
    ↓
[Feedback Loop]
    ├─ Update memory
    ├─ Check stopping criteria
    ├─ Decision: Continue or Return
    ↓
Final Output
```

### 2. Agent Framework (`crates/agent`)

**Purpose**: Provides foundational agent capabilities with skill management and tool integration.

**Key Responsibilities**:
- Agent lifecycle management
- Skill composition and execution
- Tool registry and dispatch
- Session state management
- Error handling and recovery

**Architecture** (actual tree):
```
crates/agent/src
├── agent/
│   ├── agentic/        # Agent assembly
│   │   ├── mod.rs      # Agent struct, config, session/metrics, MCP wiring
│   │   ├── llm.rs      # LlmProvider + build_vendor
│   │   ├── adapters.rs # ExtensibleToolAdapter (sync + async)
│   │   ├── engine.rs   # prepare_context / build_react_engine / react / stream
│   │   └── tests.rs
│   ├── config.rs       # TOML agent config builder
│   ├── context.rs      # AgentReactContext / AgentReActApp / AgentSession
│   ├── hooks.rs        # AgentHook lifecycle seam
│   └── plugin.rs       # AgentPlugin middleware seam
├── tools/              # ToolRegistry, FunctionTool, BashTool, validator, translator
├── skills/             # SkillLoader, SkillMetadata
├── mcp/                # MCP client, protocol, stdio/HTTP transports
├── bus/                # agent RPC: wire / transport / client / server / tool
├── session/            # SessionManager
├── security/           # WorkspaceValidator
├── metrics.rs          # CallMetrics
└── error.rs            # AgentError (re-exports react's LlmError + ToolError)
```

**Design Patterns**:
- **Registry Pattern**: Tool and skill registration
- **Factory Pattern**: Agent and session creation
- **Strategy Pattern**: Tool execution strategies
- **Decorator Pattern**: Circuit breaker wrapping tools

### 3. Event Bus (`crates/bus`)

**Purpose**: High-performance pub/sub messaging backbone for system communication.

**Key Responsibilities**:
- Async message publishing
- Event subscription and routing
- Request/response (queryable) patterns
- Session scoping
- Distributed messaging via Zenoh

**Architecture**:
```
Bus System
├── publisher.rs        # Publish-only interface
├── subscriber.rs       # Subscribe & receive
├── queryable.rs        # Request/response pattern
├── callable.rs         # Event handler traits
├── query.rs            # Query message types
├── session.rs          # Session-scoped communication
└── codec.rs            # Message serialization
```

**Communication Patterns**:

1. **Pub/Sub**:
```
Publisher --publishes--> Topic ←--subscribes-- Subscriber
```

2. **Request/Response (Queryable)**:
```
Requester --sends-query--> Topic ←--responds-- Responder
                ↓
          Awaits response
                ↓
          Response received
```

3. **Session-Scoped**:
```
Session {
  Publisher → Topic (namespaced)
  Subscriber → Topic (namespaced)
  Queryable → Query (namespaced)
}
```

### 4. Configuration System (`crates/config`)

**Purpose**: Flexible configuration loading and management.

**Key Responsibilities**:
- Load TOML, YAML configurations
- Environment variable overrides
- Configuration validation
- Glob pattern file discovery
- Type-safe configuration schemas

**Architecture**:
```
crates/config/src
├── lib.rs             # public surface + Section API
├── loader.rs          # file discovery, parsing, env overrides
├── types.rs           # configuration data types
└── error.rs           # configuration errors
```

**Loading Priority**:
```
1. Default values (hardcoded)
2. File-based config (TOML/YAML)
3. Environment variables (override)
4. Runtime modifications
```

### 5. Logging & Telemetry (`crates/logging`)

**Purpose**: Centralized structured logging and observability.

**Key Responsibilities**:
- Tracing span management
- Structured event logging
- Performance metrics collection
- Distributed trace propagation
- Integration with external observability systems

**Architecture**:
```
crates/logging/src
└── lib.rs              # init_tracing / auto_init_tracing + flexi_logger setup
```

**Tracing Levels**:
- `TRACE` - Detailed function calls
- `DEBUG` - Detailed information
- `INFO` - General information
- `WARN` - Warning conditions
- `ERROR` - Error conditions

---

## 🔄 Data Flow & Interactions

### Agent Execution Flow

```
┌─────────────────────────────────┐
│  User Request / Application     │
└──────────┬──────────────────────┘
           │
           ▼
┌─────────────────────────────────┐
│  ReAct Engine.run()             │
│  ├─ Initialize execution context│
│  ├─ Load memory                 │
│  └─ Start reasoning loop        │
└──────────┬──────────────────────┘
           │
    ┌──────┴──────────────────────┐
    │  LOOP (max iterations)       │
    ▼                              │
┌─────────────────────────────────┐│
│  LLM Call (Reasoning Phase)      ││
│  ├─ Format prompt with context  ││
│  ├─ Call LLM provider (timeout) ││
│  ├─ Parse response              ││
│  └─ Extract thought/action      ││
└──────────┬──────────────────────┘│
           │                       │
           ▼                       │
┌─────────────────────────────────┐│
│  Action Dispatch (Acting Phase) ││
│  ├─ Validate action             ││
│  ├─ Check tool registry         ││
│  ├─ Execute tool (with timeout) ││
│  │  ├─ Circuit breaker check    ││
│  │  ├─ Cache lookup             ││
│  │  └─ Tool.execute()           ││
│  └─ Capture result              ││
└──────────┬──────────────────────┘│
           │                       │
           ▼                       │
┌─────────────────────────────────┐│
│  Memory Update                  ││
│  ├─ Store interaction           ││
│  ├─ Update conversation history ││
│  └─ Persist to disk (if enabled)││
└──────────┬──────────────────────┘│
           │                       │
           ▼                       │
│  Decision: Continue or Return?  │
│  ├─ Check stopping criteria     │
│  ├─ Check iteration count       │
│  └─ Check for final response    │
└────────┬─────────────┬──────────┘
         │             │
    Continue          Return
         │             │
         └──────┬──────┘
                │
                ▼
        ┌───────────────────┐
        │  Return Result    │
        │  ├─ Final response │
        │  ├─ Reasoning path │
        │  └─ Interactions   │
        └───────────────────┘
```

### Tool Execution Flow

```
┌──────────────────────────────────┐
│  Agent: Execute Tool             │
└──────────┬───────────────────────┘
           │
           ▼
┌──────────────────────────────────┐
│  Tool Registry.get(tool_name)    │
└──────────┬───────────────────────┘
           │
           ├─ Tool not found? ──→ Error
           │
           ▼
┌──────────────────────────────────┐
│  Circuit Breaker Check           │
│  ├─ State: Closed/Open/Half-Open│
│  └─ Decision: Proceed/Fail Fast  │
└──────────┬───────────────────────┘
           │
           ├─ Open? ──→ Return cached error
           │
           ▼
┌──────────────────────────────────┐
│  Tool Cache Lookup               │
│  ├─ Hash input parameters        │
│  └─ Check cache store            │
└──────────┬───────────────────────┘
           │
           ├─ Cache hit? ──→ Return cached result
           │
           ▼
┌──────────────────────────────────┐
│  Execute Tool (with Timeout)     │
│  ├─ Spawn async task             │
│  ├─ Set timeout limit            │
│  └─ Await result                 │
└──────────┬───────────────────────┘
           │
           ├─ Timeout? ──→ Circuit breaker incident
           │
           ▼
┌──────────────────────────────────┐
│  Cache Result & Return           │
│  ├─ Store in cache               │
│  ├─ Update circuit breaker state │
│  └─ Return to agent              │
└──────────────────────────────────┘
```

### Event Bus Message Flow

```
Producer/Publisher
        │
        │ publishes event
        ▼
    ┌────────────────┐
    │  Topic Routing │
    │  (Event Filter)│
    └────────┬───────┘
             │
    ┌────────┴──────────────────┐
    │                           │
    ▼                           ▼
 Subscriber A              Subscriber B
    │                           │
    ├─ Process event            ├─ Process event
    ├─ May publish new event    ├─ May publish new event
    └─ May query others         └─ May query others

Queryable (Request/Response)
    │
    ├─ Receive query on topic
    │
    ├─ Process and generate response
    │
    └─ Publish response back to requester
```

---

## 🏗️ Design Patterns

### 1. Registry Pattern

**Used in**: Tool registry, Skill registry

```rust
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn register(&mut self, name: &str, tool: Arc<dyn Tool>) {}
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {}
}
```

### 2. Factory Pattern

**Used in**: Agent creation, Session creation

```rust
pub struct AgentFactory;

impl AgentFactory {
    pub async fn create(config: AgentConfig) -> Result<Agent> {}
}
```

### 3. Strategy Pattern

**Used in**: Tool execution strategies, LLM provider selection

```rust
pub trait ToolExecutor {
    async fn execute(&self, input: Input) -> Result<Output>;
}

pub struct DirectExecutor;
pub struct CachedExecutor;
pub struct CircuitBreakerExecutor;
```

### 4. Decorator Pattern

**Used in**: Wrapping tools with resilience features

```rust
pub struct CircuitBreakerTool<T> {
    inner: Arc<T>,
    breaker: CircuitBreaker,
}

impl Tool for CircuitBreakerTool<T> {
    async fn execute(&self, input: Input) -> Result<Output> {
        self.breaker.call(|| self.inner.execute(input)).await
    }
}
```

### 5. Observer Pattern

**Used in**: Event bus subscriptions, Telemetry hooks

```rust
pub trait Subscriber {
    async fn on_event(&self, event: Event);
}
```

### 6. State Pattern

**Used in**: Agent execution state, Circuit breaker states

```rust
pub enum CircuitBreakerState {
    Closed,
    Open { since: Instant },
    HalfOpen,
}
```

---

## 🔌 Integration Points

### 1. LLM Provider Integration

**Interface**:
```rust
pub trait LLMProvider: Send + Sync {
    async fn complete(&self, request: CompleteRequest) -> Result<CompleteResponse>;
    async fn stream(&self, request: CompleteRequest) -> Result<Stream<Chunk>>;
}
```

**Implementations**:
- OpenAI (GPT-4, GPT-3.5)
- Anthropic (Claude)
- Local models (Ollama, LLaMA)

### 2. Tool Integration

**Interface**:
```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    async fn execute(&self, input: ToolInput) -> Result<ToolOutput>;
}
```

**Built-in Tools**:
- HTTP Client
- Search
- Calculator
- Text Processor

### 3. Memory Integration

**Persistence Layer**:
```rust
pub trait MemoryBackend: Send + Sync {
    async fn save(&self, key: &str, data: &[u8]) -> Result<()>;
    async fn load(&self, key: &str) -> Result<Option<Vec<u8>>>;
}
```

**Implementations**:
- In-memory (cache)
- File system
- Database (extensible)

### 4. Bus Integration

**Communication Interfaces**:
```rust
pub trait Publisher {
    async fn publish<T: Serialize>(&self, topic: &str, message: T) -> Result<()>;
}

pub trait Subscriber {
    fn subscribe<T: DeserializeOwned>(&mut self, topic: &str) -> Receiver<T>;
}

pub trait Queryable {
    async fn query<Req, Res>(&self, topic: &str, request: Req) -> Result<Res>;
}
```

---

## 📊 Concurrency Model

### Async Runtime
- **Runtime**: Tokio (multi-threaded, work-stealing)
- **Model**: Async/await with Future-based composition
- **Parallelism**: Task spawning for concurrent tool execution

### Key Concurrency Patterns

1. **Tool Parallel Execution**:
```rust
let results = futures::future::join_all(vec![
    tool1.execute(input1),
    tool2.execute(input2),
    tool3.execute(input3),
]).await;
```

2. **Session Isolation**:
```rust
// Each session runs in its own async task
tokio::spawn(async move {
    agent.execute(request).await
});
```

3. **Channel-based Communication**:
```rust
let (tx, rx) = async_channel::bounded(100);
publisher.subscribe(topic, tx);
```

---

## 🛡️ Resilience & Fault Tolerance

### 1. Circuit Breaker Pattern

**States**:
- **Closed**: Normal operation, requests flow
- **Open**: Failures detected, requests fail fast
- **Half-Open**: Testing recovery, limited requests allowed

**Configuration**:
```rust
pub struct CircuitBreakerConfig {
    failure_threshold: u32,
    success_threshold: u32,
    timeout: Duration,
}
```

### 2. Timeout Management

- **LLM Call Timeouts**: Configurable per engine instance
- **Tool Execution Timeouts**: Per-tool configuration
- **Query Timeouts**: Bus-level timeout enforcement

### 3. Retry Strategy

- **Exponential Backoff**: Configurable base and multiplier
- **Jitter**: Random delay to avoid thundering herd
- **Max Retries**: Per-operation limit

### 4. Fallback & Graceful Degradation

- **Tool Fallbacks**: Alternative tools specified
- **Cached Results**: Use stale cache on failure
- **Partial Results**: Return partial response on timeout

---

## 📈 Performance Considerations

### 1. Memory Optimization

- **Tool Result Caching**: LRU cache with TTL
- **Stream Processing**: Avoid buffering entire responses
- **Zero-copy Serialization**: rkyv for fast deserialization

### 2. Concurrency Tuning

- **Tokio Thread Pool**: Configurable worker threads
- **Task Spawning**: Selective spawning to avoid overhead
- **Channel Bounds**: Prevent unbounded queue growth

### 3. LLM Call Optimization

- **Prompt Caching**: Template reuse and memoization
- **Streaming Responses**: Process tokens as they arrive
- **Context Pruning**: Remove old conversation turns

### 4. Observability

- **Tracing Spans**: Hierarchical request tracing
- **Metrics Collection**: Histogram of operation latencies
- **Sampling**: Reduce overhead on high-volume operations

---

## 🔐 Security Considerations

### 1. Tool Policy Enforcement

```rust
pub trait ToolPolicy {
    fn can_execute(&self, tool_name: &str, context: &Context) -> bool;
    fn sanitize_input(&self, input: ToolInput) -> ToolInput;
    fn redact_output(&self, output: ToolOutput) -> ToolOutput;
}
```

### 2. Input Validation

- Schema validation for tool inputs
- Size limits on requests/responses
- Prompt injection protection

### 3. Rate Limiting

- Per-tool rate limits
- Per-session rate limits
- Distributed rate limiting via bus

### 4. Audit Logging

- All tool executions logged
- Policy violations tracked
- Query/response patterns recorded

---

## 📚 Component Dependencies

Layers flow downward. Lower layers never depend on higher ones.

```
bindings        nbos (pyo3)        jsbos (napi)
                    └──────┬───────────┘
                           ▼
agent           agent  ──►  react, bus, config, logging, qserde
                           │
react           react  ──►  bus, qserde
                           │
infrastructure  bus    ──►  logging
                logging──►  config
                config ──►  (serde only)
                qserde ──►  qserde_derive

independent     resource   (feature-gated bus/config/logging; nothing depends on it)
```

**Layer rule**: `react` is the reasoning/LLM layer; `agent` composes it;
`nbos`/`jsbos` only marshal across an FFI seam. `resource` is an independent
subsystem (QUIC transport, Rego policy, `rex` CLI) that is not yet wired into
the agent graph.

---

### Layer ownership

| Crate | Owns | Must not |
|---|---|---|
| `qserde` | multi-backend (de)serialization | know about agents or LLMs |
| `bus` | transport, pub/sub, query/RPC, codecs | know about agents or LLMs |
| `config` | file/env configuration | depend on runtime crates |
| `logging` | tracing initialization | depend on runtime crates |
| `react` | ReAct loop, LLM providers, tool trait, resilience | depend on `agent` |
| `agent` | composition: hooks, plugins, skills, MCP, sessions | depend on bindings |
| `nbos`/`jsbos` | FFI marshalling only | hold business logic |
| `resource` | external resource transport/policy (isolated) | be imported without a declared seam |

---

## 🔄 Extension Points

### 1. Custom LLM providers

Implement `react::llm::LlmClient` and register it with `LlmProvider::register_vendor`
(or `LlmRouter`). OpenAI-compatible providers should delegate to `OpenAiVendor`;
`deepseek`, `nvidia`, and `openrouter` are ~200-line config adapters that show the pattern.

### 2. Custom tools

Implement `react::tool::Tool` (sync) or `react::tool::registry::AsyncTool` (async) and
register it in `agent::tools::ToolRegistry`.

### 3. Lifecycle hooks

Implement `AgentHook` and add it to `HookRegistry` to observe or veto agent, LLM, and
tool events.

### 4. Middleware

Implement `AgentPlugin` and add it to `PluginRegistry` to wrap LLM requests/responses,
stream tokens, and tool calls.

### 5. Bus participants

Use `bus::Bus` for publishers/subscribers/queryables, or implement `bus::Callable` for
RPC servers. Agent RPC exposes `AgentRpcClient` / `AgentCallableServer`.

---

## 📋 Version & Compatibility

- **Edition**: 2021
- **Min Rust**: 1.70+
- **Feature Flags**: Per-crate feature configuration
- **Breaking Changes**: Documented in CHANGELOG.md

---

**Last Updated**: 2026-10-05

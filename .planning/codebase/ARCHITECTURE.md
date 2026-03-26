# BrainOS Architecture

## System Overview

BrainOS is a distributed AI agent platform built on Rust, designed for scalable, high-performance agent orchestration using Zenoh for distributed messaging.

## Core Architectural Patterns

### Layered Architecture

```
┌─────────────────────────────────────────┐
│         Application Layer               │
│  (Agent orchestration, tool execution)  │
├─────────────────────────────────────────┤
│         Service Layer                   │
│  (LLM client, MCP integration, skills)  │
├─────────────────────────────────────────┤
│         Communication Layer             │
│  (Zenoh bus, pub/sub, query/response    │ 
│     ,callable/caller                    │
├─────────────────────────────────────────┤
│         Infrastructure Layer            │
│  (Config, logging, session management)  │
└─────────────────────────────────────────┘
```

## Component Architecture

### 1. Agent Layer (`crates/agent`)

**Purpose**: Core agent infrastructure for distributed AI agents

**Key Components**:
- **Agent**: Main agent implementation with message handling
- **LLM Client**: Interface to language model providers
- **Tools**: Tool registry and execution framework
- **Skills**: Skill loading and injection system
- **MCP**: Model Context Protocol client integration
- **Session**: Session management and persistence

**Data Flow**:
```
User Request → Agent → Tool/Skill Execution → LLM → Response
                ↓
            Session State
```

### 2. Communication Layer (`crates/bus`)

**Purpose**: Zenoh communication wrapper for distributed messaging

**Key Components**:
- **Publisher**: Message publishing
- **Subscriber**: Message subscription
- **Caller**: Query/response pattern
- **Callable**: Queryable service registration
- **Session**: Zenoh session management
- **Codec**: Serialization/deserialization

**Patterns**:
- **Pub/Sub**: Event-driven communication
- **Query/Response**: Request-response pattern
- **Zero-copy**: rkyv serialization for efficiency

### 3. Configuration Layer (`crates/config`)

**Purpose**: Configuration management with multiple format support

**Key Components**:
- **Loader**: Configuration file loading
- **Types**: Configuration type definitions
- **Error**: Configuration error handling

**Supported Formats**:
- TOML
- JSON
- YAML

### 4. Infrastructure Layer (`crates/logging`)

**Purpose**: Logging and tracing infrastructure

**Features**:
- File-based logging with rotation
- Console output
- Structured logging via tracing
- Automatic initialization

## Key Abstractions

### Agent Abstraction

```rust
pub struct Agent {
    config: AgentConfig,
    llm_client: LlmClient,
    tool_registry: ToolRegistry,
    skill_loader: SkillLoader,
    mcp_client: Option<McpClient>,
    session_manager: SessionManager,
}
```

**Responsibilities**:
- Message processing
- Tool orchestration
- Skill injection
- Session state management
- LLM interaction

### Tool Abstraction

```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    async fn execute(&self, input: Value) -> Result<Value, ToolError>;
}
```

**Tool Registry**: Centralized tool management with validation

### Skill System

**Purpose**: Dynamic capability injection

**Components**:
- **SkillLoader**: Loads skills from SKILL.md files
- **SkillInjector**: Injects skills into agent context
- **SkillMetadata**: Skill metadata and descriptions

### MCP Integration

**Purpose**: Model Context Protocol for tool/resource integration

**Components**:
- **McpClient**: MCP protocol client
- **McpToolAdapter**: Adapts MCP tools to internal tool interface
- **StdioTransport**: Stdio-based transport for MCP servers

## Data Flow Patterns

### 1. Agent Execution Flow

```
1. Receive message
2. Load session state
3. Inject relevant skills
4. Execute tools
5. Call LLM
6. Update session state
7. Return response
```

### 2. Tool Execution Flow

```
1. Validate tool input
2. Execute tool logic
3. Handle errors
4. Return result
```

### 3. Skill Injection Flow

```
1. Load skill metadata
2. Parse skill content
3. Inject into agent context
4. Execute skill logic
```

## Entry Points

### Main Entry Points

- `crates/agent/src/lib.rs` - Agent library entry
- `crates/bus/src/lib.rs` - Bus library entry
- `crates/config/src/lib.rs` - Config library entry
- `crates/logging/src/lib.rs` - Logging initialization

### Key Public APIs

**Agent**:
- `AgentBuilder` - Builder pattern for agent construction
- `AgentConfig` - Agent configuration
- `SessionManager` - Session lifecycle management

**Bus**:
- `Publisher` - Message publishing
- `Subscriber` - Message subscription
- `Caller` - Query/response client
- `Callable` - Queryable service

**Config**:
- `ConfigLoader` - Configuration loading
- `ConfigFormat` - Format enumeration

## Concurrency Model

- **Async/await**: All I/O operations use tokio async
- **Channels**: async-channel for message passing
- **Lock-free**: Where possible, use channels instead of locks
- **Session isolation**: Each agent session is isolated

## Error Handling Strategy

- **Result types**: All fallible operations return `Result<T, E>`
- **Error propagation**: Use `?` operator for error propagation
- **Context**: `anyhow` for error context in application code
- **Typed errors**: `thiserror` for library error types

## Performance Considerations

- **Zero-copy**: rkyv serialization for data transfer
- **Async I/O**: Non-blocking I/O operations
- **Connection pooling**: Reuse connections where possible
- **Lazy loading**: Skills loaded on-demand
- **Optimized builds**: LTO and single codegen unit in release

# BrainOS Agent Framework - Validation Roadmap

**Project:** BrainOS Agent Framework - Distributed LLM agent orchestration on Zenoh
**Crates:** `crates/bus`, `crates/agent`
**Goal:** Comprehensive framework validation through demos

---

## Milestone: v1.1 - Framework Validation

**Goal:** Validate ALL framework requirements through focused demos that expose API issues and demonstrate real-world usage patterns.

---

## Phase 1: Core Communication (Bus)

**Goal:** Validate Zenoh bus communication primitives — RPC, discovery, pub/sub, and query patterns.

**Requirements:** BUS-01, BUS-02, BUS-03, BUS-04

**Success Criteria:**
1. RPC service registers and responds to requests correctly
2. Service discovery finds registered services
3. Pub/sub delivers messages to subscribers
4. Query pattern works for request/response

---

### Plan 01-01: RPC Service Validation

**Demo:** `demo-rpc-service/`
- Single binary that registers multiple RPC services
- Tests different handler patterns

**Features Tested:**
- `RpcServiceBuilder` pattern (build → init)
- `RpcService::new()` pattern (new → init → announce)
- RpcHandler trait implementation
- Request/response serialization

**Test Cases:**
| Case | Input | Expected | Verification |
|------|-------|----------|--------------|
| Builder pattern | Register `add` via builder | Service responds | `cargo test demo_rpc_add` |
| New pattern | Register `echo` via new() | Service responds | `cargo test demo_rpc_echo` |
| Binary payload | Send bytes directly | Bytes returned | `cargo test demo_rpc_binary` |
| Error handling | Invalid request | Error response | `cargo test demo_rpc_error` |
| Concurrent calls | 10 parallel requests | All handled | `cargo test demo_rpc_concurrent` |

**Tasks:**

```xml
<task type="auto" tdd="true">
<name>Task 1: Create RPC service demo structure</name>
<files>examples/demo-rpc-service/Cargo.toml, examples/demo-rpc-service/src/main.rs</files>
<action>Create demo project with Cargo.toml dependening on bus crate. Create main.rs with async main that:
- Sets up Zenoh session
- Registers 3 RPC services: add (builder), echo (new), error (error handling)
- Each service has a handler that logs and responds</action>
<verify><automated>cargo check -p demo-rpc-service</automated></verify>
<done>Demo compiles with bus dependency</done>
</task>

<task type="auto" tdd="true">
<name>Task 2: Write RPC service tests</name>
<files>examples/demo-rpc-service/tests/rpc_test.rs</files>
<action>Create integration tests that:
- Test add service: call with {a: 5, b: 3} expect {result: 8}
- Test echo service: send "hello" expect "hello" back
- Test error service: send invalid data expect error response
- Test concurrent: spawn 10 tasks calling add simultaneously</action>
<verify><automated>cargo test -p demo-rpc-service 2>&1 | grep -E "(test result|passed|failed)"</automated></verify>
<done>All 5 test cases pass</done>
</task>
</tasks>

**Atomic Commits:**
1. `feat(demo-rpc): scaffold RPC service demo` — Cargo.toml, basic structure
2. `test(demo-rpc): add RPC service tests` — Test cases for all scenarios
3. `refactor(demo-rpc): fix builder vs new pattern issues` — Document API inconsistency

---

### Plan 01-02: Service Discovery Validation

**Demo:** `demo-discovery/`
- Multiple services announce themselves
- Client discovers and calls services

**Features Tested:**
- `RpcService::announce()` — Publish service info
- `RpcDiscovery` — Discover specific service
- `DiscoveryRegistry` — List all services
- Service filtering and matching

**Test Cases:**
| Case | Input | Expected | Verification |
|------|-------|----------|--------------|
| Single discovery | Announce "math" | Discover "math" | `cargo test demo_discovery_single` |
| Multiple services | Announce A, B, C | List all 3 | `cargo test demo_discovery_list` |
| Filter by prefix | Announce "agent/a/tools" | Filter works | `cargo test demo_discovery_filter` |
| Timeout | Discover non-existent | Timeout error | `cargo test demo_discovery_timeout` |
| Re-announce | Re-announce after crash | Updated info | `cargo test demo_discovery_refresh` |

**Tasks:**

```xml
<task type="auto" tdd="true">
<name>Task 1: Create discovery demo</name>
<files>examples/demo-discovery/Cargo.toml, examples/demo-discovery/src/main.rs</files>
<action>Create demo that:
- Starts 3 services: calculator, weather, echo
- Each announces itself
- Demonstrates DiscoveryRegistry.list_services()
- Shows filtering by service name prefix</action>
<verify><automated>cargo check -p demo-discovery</automated></verify>
<done>Demo compiles</done>
</task>

<task type="auto" tdd="true">
<name>Task 2: Write discovery tests</name>
<files>examples/demo-discovery/tests/discovery_test.rs</files>
<action>Create tests that:
- Test discovering a single announced service
- Test listing all services
- Test filtering services by prefix
- Test timeout on non-existent service</action>
<verify><automated>cargo test -p demo-discovery 2>&1 | grep -E "(test result|passed|failed)"</automated></verify>
<done>All tests pass</done>
</task>
</tasks>

**Atomic Commits:**
1. `feat(demo-discovery): scaffold discovery demo` — Project structure
2. `test(demo-discovery): add discovery tests` — Test coverage

---

### Plan 01-03: Pub/Sub & Query Validation

**Demo:** `demo-pubsub-query/`
- Publisher/subscriber pattern
- Query/request-response pattern

**Features Tested:**
- `PublisherWrapper` for pub/sub
- `SubscriberWrapper` for receiving
- `QueryWrapper` for request/response
- `QueryableWrapper` for responding to queries

**Test Cases:**
| Case | Input | Expected | Verification |
|------|-------|----------|--------------|
| Pub/Sub | Publish 5 messages | All received | `cargo test demo_pubsub` |
| Query/Response | Query with data | Response returned | `cargo test demo_query` |
| Wildcard subscribe | Subscribe to "agent/*" | Multiple received | `cargo test demo_wildcard` |
| Query timeout | Query non-existent | Timeout error | `cargo test demo_query_timeout` |

**Tasks:**

```xml
<task type="auto" tdd="true">
<name>Task 1: Create pub/sub demo</name>
<files>examples/demo-pubsub-query/Cargo.toml, examples/demo-pubsub-query/src/main.rs</files>
<action>Create demo that:
- Demonstrates PublisherWrapper publishing to topic
- Demonstrates SubscriberWrapper receiving messages
- Shows QueryWrapper/QueryableWrapper pattern
- Uses wildcard subscriptions</action>
<verify><automated>cargo check -p demo-pubsub-query</automated></verify>
<done>Demo compiles</done>
</task>

<task type="auto" tdd="true">
<name>Task 2: Write pub/sub tests</name>
<files>examples/demo-pubsub-query/tests/pubsub_test.rs</files>
<action>Create tests for pub/sub and query patterns</action>
<verify><automated>cargo test -p demo-pubsub-query</automated></verify>
<done>All tests pass</done>
</task>
</tasks>

**Atomic Commits:**
1. `feat(demo-pubsub): scaffold pub/sub and query demo`
2. `test(demo-pubsub): add pub/sub and query tests`

---

## Phase 2: Agent Framework

**Goal:** Validate Agent struct, tool system, and LLM client integration.

**Requirements:** AGENT-01, AGENT-02, TOOL-01, TOOL-02, TOOL-03, TOOL-04, TOOL-05, LLM-01, LLM-02

**Success Criteria:**
1. Agent constructed from config runs and produces output
2. Tool registration works with ToolRegistry
3. Tool execution returns correct results
4. LLM client (real and mock) integrates with Agent

---

### Plan 02-01: Agent Lifecycle Validation

**Demo:** `demo-agent-lifecycle/`
- Agent construction and execution
- Config-driven agent loading

**Features Tested:**
- `Agent::new()` with AgentConfig
- `Agent::run()` for single-turn
- `Agent::run_with_tools()` for multi-turn with tools
- Message history management
- Config loading from TOML

**Test Cases:**
| Case | Input | Expected | Verification |
|------|-------|----------|--------------|
| Agent construction | Create Agent from config | Agent ready | `cargo test demo_agent_new` |
| Single turn | Run "hello" | Response text | `cargo test demo_agent_single_turn` |
| Multi-turn | Run 3 messages | Context preserved | `cargo test demo_agent_multi_turn` |
| Config loading | Load from TOML | Agent created | `cargo test demo_agent_config` |
| Mock LLM | Use mock client | Predictable output | `cargo test demo_agent_mock` |

**Tasks:**

```xml
<task type="auto" tdd="true">
<name>Task 1: Create agent lifecycle demo</name>
<files>examples/demo-agent-lifecycle/Cargo.toml, examples/demo-agent-lifecycle/src/main.rs</files>
<action>Create demo that:
- Shows Agent::new() with all config options
- Demonstrates run() for simple queries
- Shows run_with_tools() for tool-augmented queries
- Includes config file example</action>
<verify><automated>cargo check -p demo-agent-lifecycle</automated></verify>
<done>Demo compiles</done>
</task>

<task type="auto" tdd="true">
<name>Task 2: Write agent lifecycle tests</name>
<files>examples/demo-agent-lifecycle/tests/agent_test.rs</files>
<action>Create tests for agent construction, single/multi-turn, config loading</action>
<verify><automated>cargo test -p demo-agent-lifecycle</automated></verify>
<done>All tests pass</done>
</task>
</tasks>

**Atomic Commits:**
1. `feat(demo-agent-lifecycle): scaffold agent lifecycle demo`
2. `test(demo-agent-lifecycle): add agent lifecycle tests`

---

### Plan 02-02: Tool System Validation

**Demo:** `demo-tool-system/`
- Tool trait implementation
- ToolRegistry operations
- Tool execution

**Features Tested:**
- `Tool` trait implementation
- `ToolRegistry::register()`
- `ToolRegistry::execute()`
- Tool description and JSON schema
- Error handling (SchemaMismatch, ExecutionFailed)

**Test Cases:**
| Case | Input | Expected | Verification |
|------|-------|----------|--------------|
| Register tool | Register AddTool | Tool in registry | `cargo test demo_tool_register` |
| Execute tool | Execute "add" with {a:1,b:2} | Result 3 | `cargo test demo_tool_execute` |
| Schema validation | Pass wrong args | SchemaMismatch | `cargo test demo_tool_schema` |
| Missing tool | Execute non-existent | Error | `cargo test demo_tool_missing` |
| Multiple tools | Register 5 tools | All usable | `cargo test demo_tool_multiple` |
| Local vs RPC tool | Both work same | Consistent | `cargo test demo_tool_local_vs_rpc` |

**Tasks:**

```xml
<task type="auto" tdd="true">
<name>Task 1: Create tool system demo</name>
<files>examples/demo-tool-system/Cargo.toml, examples/demo-tool-system/src/main.rs</files>
<action>Create demo that:
- Implements Tool trait for Add, Multiply, Divide tools
- Registers tools in ToolRegistry
- Shows both ToolRegistry execute patterns
- Demonstrates error handling for schema/execution errors</action>
<verify><automated>cargo check -p demo-tool-system</automated></verify>
<done>Demo compiles</done>
</task>

<task type="auto" tdd="true">
<name>Task 2: Write tool system tests</name>
<files>examples/demo-tool-system/tests/tool_test.rs</files>
<action>Create tests covering all tool scenarios</action>
<verify><automated>cargo test -p demo-tool-system</automated></verify>
<done>All tests pass</done>
</task>
</tasks>

**Atomic Commits:**
1. `feat(demo-tool-system): scaffold tool system demo`
2. `test(demo-tool-system): add tool system tests`

---

### Plan 02-03: LLM Client Validation

**Demo:** `demo-llm-client/`
- Real LLM integration
- Mock LLM for testing
- Streaming support

**Features Tested:**
- `LlmClient` trait
- `OpenAiClient` implementation
- `MockLlmClient` for testing
- `stream_complete()` for streaming
- Error handling (API errors, timeout)

**Test Cases:**
| Case | Input | Expected | Verification |
|------|-------|----------|--------------|
| Mock client | Use MockLlmClient | Predictable output | `cargo test demo_llm_mock` |
| Real client (mocked) | Call OpenAiClient | HTTP called | `cargo test demo_llm_real` |
| Streaming | Use stream_complete() | Tokens yielded | `cargo test demo_llm_stream` |
| Error handling | Invalid API key | Error returned | `cargo test demo_llm_error` |
| Timeout | Slow API | Timeout error | `cargo test demo_llm_timeout` |

**Tasks:**

```xml
<task type="auto" tdd="true">
<name>Task 1: Create LLM client demo</name>
<files>examples/demo-llm-client/Cargo.toml, examples/demo-llm-client/src/main.rs</files>
<action>Create demo that:
- Shows both MockLlmClient and OpenAiClient
- Demonstrates complete() and stream_complete()
- Includes mock response fixtures for testing</action>
<verify><automated>cargo check -p demo-llm-client</automated></verify>
<done>Demo compiles</done>
</task>

<task type="auto" tdd="true">
<name>Task 2: Write LLM client tests</name>
<files>examples/demo-llm-client/tests/llm_test.rs</files>
<action>Create tests for mock, real, streaming, and error scenarios</action>
<verify><automated>cargo test -p demo-llm-client</automated></verify>
<done>All tests pass</done>
</task>
</tasks>

**Atomic Commits:**
1. `feat(demo-llm-client): scaffold LLM client demo`
2. `test(demo-llm-client): add LLM client tests`

---

## Phase 3: A2A Protocol

**Goal:** Validate Agent-to-Agent communication, discovery, and task delegation.

**Requirements:** A2A-01, A2A-02, A2A-03, A2A-04

**Success Criteria:**
1. Agent identity and capability exchange works
2. Task delegation delivers task to recipient
3. Task state machine transitions correctly
4. Response routing returns to sender

---

### Plan 03-01: A2A Identity & Discovery

**Demo:** `demo-a2a-identity/`
- AgentIdentity creation
- AgentCard announcement
- Agent discovery

**Features Tested:**
- `AgentIdentity` struct (id, name, version)
- `AgentCard` with capabilities and skills
- `A2ADiscovery::announce()`
- `A2ADiscovery::discover()`
- Capability matching

**Test Cases:**
| Case | Input | Expected | Verification |
|------|-------|----------|--------------|
| Create identity | New AgentIdentity | Valid struct | `cargo test demo_a2a_identity_new` |
| Announce card | Announce AgentCard | Published to bus | `cargo test demo_a2a_announce` |
| Discover agents | Discover all | List returned | `cargo test demo_a2a_discover` |
| Filter by skill | Discover with skill filter | Filtered list | `cargo test demo_a2a_filter` |
| Re-announce | Update card | Updated info | `cargo test demo_a2a_update` |

**Tasks:**

```xml
<task type="auto" tdd="true">
<name>Task 1: Create A2A identity demo</name>
<files>examples/demo-a2a-identity/Cargo.toml, examples/demo-a2a-identity/src/main.rs</files>
<action>Create demo that:
- Creates multiple AgentIdentity instances
- Builds AgentCard with capabilities and skills
- Shows announce() and discover() patterns
- Demonstrates filtering by skill/capability</action>
<verify><automated>cargo check -p demo-a2a-identity</automated></verify>
<done>Demo compiles</done>
</task>

<task type="auto" tdd="true">
<name>Task 2: Write A2A identity tests</name>
<files>examples/demo-a2a-identity/tests/a2a_identity_test.rs</files>
<action>Create tests for identity, card, discovery</action>
<verify><automated>cargo test -p demo-a2a-identity</automated></verify>
<done>All tests pass</done>
</task>
</tasks>

**Atomic Commits:**
1. `feat(demo-a2a-identity): scaffold A2A identity demo`
2. `test(demo-a2a-identity): add A2A identity tests`

---

### Plan 03-02: Task Delegation & State Machine

**Demo:** `demo-a2a-task/`
- Task creation and delegation
- Task state machine transitions
- Response handling

**Features Tested:**
- `Task` struct (task_id, input, output, state)
- `TaskState` enum (Submitted, Working, Completed, Failed, InputRequired)
- `A2AClient::delegate_task()`
- Response topic routing
- Timeout handling

**Test Cases:**
| Case | Input | Expected | Verification |
|------|-------|----------|--------------|
| Create task | New Task | Valid struct | `cargo test demo_task_new` |
| Delegate task | Send to recipient | Delivered | `cargo test demo_task_delegate` |
| State transitions | Submit → Working → Complete | States correct | `cargo test demo_task_state` |
| Response routing | Task response | Returns to sender | `cargo test demo_task_response` |
| Timeout | No response | Timeout error | `cargo test demo_task_timeout` |
| Failed task | Handler error | Failed state | `cargo test demo_task_failure` |

**Tasks:**

```xml
<task type="auto" tdd="true">
<name>Task 1: Create A2A task demo</name>
<files>examples/demo-a2a-task/Cargo.toml, examples/demo-a2a-task/src/main.rs</files>
<action>Create demo that:
- Shows Task struct with all fields
- Demonstrates delegate_task()
- Implements task handler with state transitions
- Shows response topic routing</action>
<verify><automated>cargo check -p demo-a2a-task</automated></verify>
<done>Demo compiles</done>
</task>

<task type="auto" tdd="true">
<name>Task 2: Write A2A task tests</name>
<files>examples/demo-a2a-task/tests/a2a_task_test.rs</files>
<action>Create tests for task creation, delegation, state machine</action>
<verify><automated>cargo test -p demo-a2a-task</automated></verify>
<done>All tests pass</done>
</task>
</tasks>

**Atomic Commits:**
1. `feat(demo-a2a-task): scaffold A2A task demo`
2. `test(demo-a2a-task): add A2A task tests`

---

### Plan 03-03: End-to-End A2A Demo

**Demo:** `demo-a2a-complete/`
- Full Alice/Bob conversation
- Combines all A2A concepts

**Features Tested:**
- Complete agent workflow
- Cross-agent tool calls via RPC
- A2A task delegation
- Tool discovery

**Test Cases:**
| Case | Input | Expected | Verification |
|------|-------|----------|--------------|
| Hello exchange | Alice → Bob "hi" | Response received | `cargo test demo_a2a_hello` |
| Tool call via A2A | Delegate calculation | Result returned | `cargo test demo_a2a_calculate` |
| Multi-turn | 3 messages | Context maintained | `cargo test demo_a2a_multiturn` |

**Tasks:**

```xml
<task type="auto">
<name>Task 1: Create complete A2A demo</name>
<files>examples/demo-a2a-complete/Cargo.toml, examples/demo-a2a-complete/src/main.rs</files>
<action>Create complete demo combining:
- Agent identity and card
- Service discovery
- Task delegation
- Tool execution across agents</action>
<verify><automated>cargo check -p demo-a2a-complete</automated></verify>
<done>Demo compiles</done>
</task>

<task type="auto">
<name>Task 2: Write complete A2A tests</name>
<files>examples/demo-a2a-complete/tests/a2a_complete_test.rs</files>
<action>Integration tests for full A2A workflows</action>
<verify><automated>cargo test -p demo-a2a-complete</automated></verify>
<done>All tests pass</done>
</task>
</tasks>

**Atomic Commits:**
1. `feat(demo-a2a-complete): scaffold complete A2A demo`
2. `test(demo-a2a-complete): add complete A2A tests`

---

## Phase 4: Advanced Features

**Goal:** Validate streaming, scheduler, skills, and MCP integration.

**Requirements:** STRM-01, STRM-02, STRM-03, SCHD-01, SCHD-02, SCHD-03, SCHD-04, SKIL-01, SKIL-02, SKIL-03, SKIL-04, MCP-01, MCP-02, MCP-03

**Success Criteria:**
1. Streaming tokens arrive incrementally
2. Workflows execute (sequential, parallel, conditional)
3. Skills load and compose
4. MCP tools integrate

---

### Plan 04-01: Streaming Validation

**Demo:** `demo-streaming/`
- Token streaming over SSE
- Token streaming over bus
- Backpressure handling

**Features Tested:**
- `SseDecoder` for HTTP streaming
- `TokenPublisher` for bus streaming
- `RateLimiter` for backpressure
- `BackpressureController`

**Test Cases:**
| Case | Input | Expected | Verification |
|------|-------|----------|--------------|
| SSE decode | Parse SSE stream | Tokens yielded | `cargo test demo_sse_decode` |
| Token publisher | Publish tokens | Received | `cargo test demo_token_publish` |
| Rate limiter | 1000 req/s | Throttled | `cargo test demo_rate_limit` |
| Backpressure | Burst of data | Buffered | `cargo test demo_backpressure` |

**Tasks:**

```xml
<task type="auto" tdd="true">
<name>Task 1: Create streaming demo</name>
<files>examples/demo-streaming/Cargo.toml, examples/demo-streaming/src/main.rs</files>
<action>Create demo showing:
- SSE token decoding
- TokenPublisher usage
- RateLimiter configuration
- BackpressureController setup</action>
<verify><automated>cargo check -p demo-streaming</automated></verify>
<done>Demo compiles</done>
</task>

<task type="auto" tdd="true">
<name>Task 2: Write streaming tests</name>
<files>examples/demo-streaming/tests/streaming_test.rs</files>
<action>Create tests for all streaming scenarios</action>
<verify><automated>cargo test -p demo-streaming</automated></verify>
<done>All tests pass</done>
</task>
</tasks>

**Atomic Commits:**
1. `feat(demo-streaming): scaffold streaming demo`
2. `test(demo-streaming): add streaming tests`

---

### Plan 04-02: Scheduler Validation

**Demo:** `demo-scheduler/`
- Sequential workflow
- Parallel workflow
- Conditional branching
- Retry with backoff

**Features Tested:**
- `Workflow` struct
- `Step` with StepType (Sequential, Parallel, Conditional)
- `WorkflowBuilder` DSL
- `Scheduler::execute()`
- `BackoffStrategy`

**Test Cases:**
| Case | Input | Expected | Verification |
|------|-------|----------|--------------|
| Sequential | 3 steps A→B→C | A→B→C order | `cargo test demo_sched_seq` |
| Parallel | 3 steps parallel | All complete | `cargo test demo_sched_par` |
| Conditional | Branch on output | Correct branch | `cargo test demo_sched_cond` |
| Retry | Fail 2 times then succeed | 3 attempts | `cargo test demo_sched_retry` |
| Timeout | Step takes too long | Timeout error | `cargo test demo_sched_timeout` |

**Tasks:**

```xml
<task type="auto" tdd="true">
<name>Task 1: Create scheduler demo</name>
<files>examples/demo-scheduler/Cargo.toml, examples/demo-scheduler/src/main.rs</files>
<action>Create demo showing:
- WorkflowBuilder for sequential/parallel/conditional
- Scheduler::execute()
- BackoffStrategy configuration
- Step timeout handling</action>
<verify><automated>cargo check -p demo-scheduler</automated></verify>
<done>Demo compiles</done>
</task>

<task type="auto" tdd="true">
<name>Task 2: Write scheduler tests</name>
<files>examples/demo-scheduler/tests/scheduler_test.rs</files>
<action>Create tests for all workflow patterns</action>
<verify><automated>cargo test -p demo-scheduler</automated></verify>
<done>All tests pass</done>
</task>
</tasks>

**Atomic Commits:**
1. `feat(demo-scheduler): scaffold scheduler demo`
2. `test(demo-scheduler): add scheduler tests`

---

### Plan 04-03: Skills & MCP Validation

**Demo:** `demo-skills-mcp/`
- Skill loading from YAML
- Skill composition
- MCP tool adapter

**Features Tested:**
- `SkillLoader` with lazy discovery
- `SkillInjector` for context injection
- `SkillMetadata` types
- `McpToolAdapter` conversion
- `McpClient` for MCP servers

**Test Cases:**
| Case | Input | Expected | Verification |
|------|-------|----------|--------------|
| Load skill | Load from YAML | Skill loaded | `cargo test demo_skill_load` |
| Compose skills | Load 2 skills | No conflicts | `cargo test demo_skill_compose` |
| Inject skill | Inject into prompt | Injected | `cargo test demo_skill_inject` |
| MCP adapter | MCP tool → Tool | Conversion works | `cargo test demo_mcp_adapter` |
| MCP client | Connect to server | Tools available | `cargo test demo_mcp_client` |

**Tasks:**

```xml
<task type="auto" tdd="true">
<name>Task 1: Create skills/MCP demo</name>
<files>examples/demo-skills-mcp/Cargo.toml, examples/demo-skills-mcp/src/main.rs, examples/demo-skills-mcp/skills/*.yaml</files>
<action>Create demo showing:
- SkillLoader usage
- SkillInjector for prompt composition
- McpToolAdapter conversion
- McpClient connection</action>
<verify><automated>cargo check -p demo-skills-mcp</automated></verify>
<done>Demo compiles</done>
</task>

<task type="auto" tdd="true">
<name>Task 2: Write skills/MCP tests</name>
<files>examples/demo-skills-mcp/tests/skills_mcp_test.rs</files>
<action>Create tests for skills and MCP</action>
<verify><automated>cargo test -p demo-skills-mcp</automated></verify>
<done>All tests pass</done>
</task>
</tasks>

**Atomic Commits:**
1. `feat(demo-skills-mcp): scaffold skills/MCP demo`
2. `test(demo-skills-mcp): add skills/MCP tests`

---

## Phase 5: Ergonomics Validation

**Goal:** Identify and validate improvements for developer experience.

**Requirements:** ERGO-01, ERGO-02, ERGO-03, ERGO-04

**Success Criteria:**
1. Boilerplate reduction achieved
2. API consistency across modules
3. Error handling is structured
4. Documentation is complete

---

### Plan 05-01: Boilerplate Reduction

**Focus:** Address issues from API analysis
- BobToolInvoker (80 lines → 5 lines)
- Tool discovery filtering (25 lines → 5 lines)
- Serialization boilerplate

**Issues to Fix:**
1. Create `RpcToolInvoker` helper in agent crate
2. Create `DiscoveryFilter` helper
3. Create `JsonPayload` utility

**Test Cases:**
| Case | Before | After | Verification |
|------|--------|-------|--------------|
| Tool invoker | 80 lines | 20 lines | Code comparison |
| Discovery filter | 25 lines | 10 lines | Code comparison |
| Serialization | 3 formats | 1 format | Single path |

**Tasks:**

```xml
<task type="auto">
<name>Task 1: Create RpcToolInvoker helper</name>
<files>crates/agent/src/tools/rpc_invoker.rs</files>
<action>Create generic RpcToolInvoker<T> that:
- Implements Tool trait
- Takes service name, topic, and deserialization target
- Handles rkyv→JSON→rkyv internally
- Reduces 80 lines to ~20 lines</action>
<verify><automated>cargo check -p agent && cargo test -p agent rpc_invoker</automated></verify>
<done>Helper compiles and tests pass</done>
</task>

<task type="auto">
<name>Task 2: Create DiscoveryFilter helper</name>
<files>crates/agent/src/a2a/discovery_filter.rs</files>
<action>Create DiscoveryFilter that:
- Wraps DiscoveryRegistry
- Provides filter_by_agent(), filter_by_skill(), filter_by_capability()
- Reduces 25 lines to ~10 lines</action>
<verify><automated>cargo check -p agent && cargo test -p agent discovery_filter</automated></verify>
<done>Helper compiles and tests pass</done>
</task>

<task type="auto">
<name>Task 3: Simplify serialization path</name>
<files>crates/bus/src/codec.rs</files>
<action>Add JsonCodec that handles JSON→JSON directly for app-level payloads, reducing triple serialization</action>
<verify><automated>cargo check -p bus</automated></verify>
<done>JsonCodec available</done>
</task>
</tasks>

**Atomic Commits:**
1. `feat(agent): add RpcToolInvoker helper` — Reduces boilerplate
2. `feat(agent): add DiscoveryFilter helper` — Simplifies filtering
3. `feat(bus): add JsonCodec for single-serialization` — Reduces complexity

---

### Plan 05-02: API Consistency

**Focus:** Fix dual constructor patterns and naming confusion

**Issues to Fix:**
1. `RpcService::new()` vs `RpcServiceBuilder` → Single builder pattern
2. `RpcDiscovery.announce()` should be `ServiceAnnouncer`
3. `DiscoveryRegistry` should be `ServiceDiscovery`

**Test Cases:**
| Case | Current | After | Verification |
|------|---------|-------|--------------|
| Service creation | 2 patterns | 1 pattern | Builder only |
| Discovery naming | Confusing | Clear | Rename types |

**Tasks:**

```xml
<task type="auto">
<name>Task 1: Deprecate RpcService::new() pattern</name>
<files>crates/bus/src/rpc/service.rs</files>
<action>Mark RpcService::new() as deprecated, update docs to recommend builder pattern only</action>
<verify><automated>cargo check -p bus 2>&1 | grep -i deprec</automated></verify>
<done>Deprecation warning shown</done>
</task>

<task type="auto">
<name>Task 2: Add type aliases for clarity</name>
<files>crates/bus/src/rpc/discovery.rs</files>
<action>Add:
- `type ServiceAnnouncer = RpcDiscovery;`
- `type ServiceDiscovery = DiscoveryRegistry;`
- Document when to use each</action>
<verify><automated>cargo check -p bus</automated></verify>
<done>Type aliases available</done>
</task>
</tasks>

**Atomic Commits:**
1. `refactor(bus): deprecate RpcService::new() in favor of builder`
2. `refactor(bus): add type aliases for discovery clarity`

---

### Plan 05-03: Structured Error Handling

**Focus:** Convert string errors to structured errors

**Issues to Fix:**
- `ToolError(String)` → `ToolError { code, message, context }`
- `RpcServiceError(String)` → `RpcServiceError { code, message, details }`

**Test Cases:**
| Case | Input | Expected | Verification |
|------|-------|----------|--------------|
| Tool error code | ExecutionFailed | Error code present | `cargo test demo_error_tool` |
| Rpc error code | InvalidRequest | Error code present | `cargo test demo_error_rpc` |
| Error context | Nested error | Context preserved | `cargo test demo_error_context` |

**Tasks:**

```xml
<task type="auto">
<name>Task 1: Enhance ToolError structure</name>
<files>crates/agent/src/error.rs</files>
<action>Expand ToolError enum:
- Add error code field
- Add context field for chain
- Add From implementations</action>
<verify><automated>cargo check -p agent && cargo test -p agent error</automated></verify>
<done>Structured ToolError available</done>
</task>

<task type="auto">
<name>Task 2: Enhance RpcServiceError structure</name>
<files>crates/bus/src/rpc/error.rs</files>
<action>Expand RpcServiceError with code, context</action>
<verify><automated>cargo check -p bus && cargo test -p bus error</automated></verify>
<done>Structured RpcServiceError available</done>
</task>
</tasks>

**Atomic Commits:**
1. `refactor(agent): add structured error codes to ToolError`
2. `refactor(bus): add structured error codes to RpcServiceError`

---

## Summary

| Phase | Plans | Focus | Complexity |
|-------|-------|-------|------------|
| 1 | 3 | Core Communication (Bus) | Medium |
| 2 | 3 | Agent Framework | Medium |
| 3 | 3 | A2A Protocol | Medium |
| 4 | 3 | Advanced Features | High |
| 5 | 3 | Ergonomics | Medium |
| **Total** | **15** | | |

---

## Requirements Mapping

| Requirement | Description | Phase | Plan |
|-------------|-------------|-------|------|
| BUS-01 | RPC service registration/execution | 1 | 01-01 |
| BUS-02 | Service discovery/announcement | 1 | 01-02 |
| BUS-03 | Pub/sub messaging | 1 | 01-03 |
| BUS-04 | Query-based communication | 1 | 01-03 |
| AGENT-01 | Agent construction from config | 2 | 02-01 |
| AGENT-02 | Agent execution (run, run_with_tools) | 2 | 02-01 |
| TOOL-01 | Tool trait implementation | 2 | 02-02 |
| TOOL-02 | ToolRegistry operations | 2 | 02-02 |
| TOOL-03 | Tool execution | 2 | 02-02 |
| TOOL-04 | Schema validation | 2 | 02-02 |
| TOOL-05 | Tool description | 2 | 02-02 |
| LLM-01 | LlmClient trait | 2 | 02-03 |
| LLM-02 | OpenAiClient + MockLlmClient | 2 | 02-03 |
| A2A-01 | AgentIdentity + AgentCard | 3 | 03-01 |
| A2A-02 | Task delegation | 3 | 03-02 |
| A2A-03 | Task state machine | 3 | 03-02 |
| A2A-04 | Response routing | 3 | 03-02 |
| STRM-01 | Token streaming | 4 | 04-01 |
| STRM-02 | Streaming over bus | 4 | 04-01 |
| STRM-03 | Backpressure handling | 4 | 04-01 |
| SCHD-01 | Sequential workflow | 4 | 04-02 |
| SCHD-02 | Parallel workflow | 4 | 04-02 |
| SCHD-03 | Conditional branching | 4 | 04-02 |
| SCHD-04 | Retry with backoff | 4 | 04-02 |
| SKIL-01 | Skill loading | 4 | 04-03 |
| SKIL-02 | Skill composition | 4 | 04-03 |
| SKIL-03 | Skill injection | 4 | 04-03 |
| SKIL-04 | Skill conflicts | 4 | 04-03 |
| MCP-01 | MCP client | 4 | 04-03 |
| MCP-02 | MCP tool adapter | 4 | 04-03 |
| MCP-03 | MCP bridge | 4 | 04-03 |
| ERGO-01 | Boilerplate reduction | 5 | 05-01 |
| ERGO-02 | API consistency | 5 | 05-02 |
| ERGO-03 | Structured errors | 5 | 05-03 |
| ERGO-04 | Documentation | 5 | All |

---

## Next Steps

1. Start with **Phase 1** — Core Communication
2. Build demos incrementally, testing each component
3. Use TDD approach: Write test → See it fail → Implement → Pass
4. After Phase 5, create consolidated `examples/llm-agent-demo-v2` that uses all improvements

---

*Created: 2026-03-20*
*Status: Ready for execution*
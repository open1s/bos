# Features Research

**Domain:** Distributed LLM Agent Orchestration Framework (Rust/Zenoh)
**Researched:** 2026-03-19
**Confidence:** MEDIUM-HIGH

## Feature Categories

### Table Stakes (Must Have — Users Expect These)

#### Agent Primitive
**AGENT-01 aligned.** An agent must:
- Be constructable from config (name, system prompt, LLM client config)
- Maintain message history (conversation turns)
- Execute a reasoning loop: receive task → call LLM → parse response → handle tools/exit
- Return structured responses

**What users expect:** "I define an agent, give it a task, get a result."

#### Tool Calling
**AGENT-02 aligned.** Agents must be able to call functions:
- Define tools with name, description, JSON Schema arguments
- LLM decides when to call tools based on schema
- Execute tool, return result to LLM for next turn
- Support multiple tools simultaneously

**What users expect:** "The agent can use tools, not just chat."

#### LLM Provider Abstraction
**AGENT-01 aligned.** Must work with:
- OpenAI-compatible APIs (OpenAI, Azure OpenAI, local proxies)
- Anthropic Claude (different tool format)
- At minimum: OpenAI + one other provider

**What users expect:** "I can switch LLM providers without rewriting agent code."

#### Streaming (Basic)
**AGENT-08 aligned.** Minimum:
- Receive tokens as they're generated
- Yield via async stream
- Graceful handling of provider streaming differences

**What users expect:** "I see output as it's generated, not all at once."

#### Config-Driven Setup
**AGENT-09 aligned.** Define from config:
- Agent definitions (name, model, system prompt)
- Tool registrations
- Skill attachments

**What users expect:** "I configure agents via TOML, not Rust code."

### Differentiators (Competitive Advantage)

#### Distributed Agent Communication (A2A)
**AGENT-04 aligned.** Agents discover each other via Zenoh and delegate:
- One agent hands off a subtask to another
- Task state tracked across agents (submitted → working → completed/failed)
- No central coordinator — peer-to-peer via bus

**Why it's a differentiator:** Most agent frameworks are single-agent or require a central orchestration service. Bus-native A2A means zero-configuration multi-agent.

#### Skills System
**AGENT-05 aligned.** Composable capability modules:
- Loadable from config
- Composable (agents can have multiple skills)
- Skill = prompt fragment + associated tools
- Namespace to avoid conflicts

**Why it's a differentiator:** Compared to LangChain's chains, skills are lighter, config-driven, and composable without code changes.

#### MCP Integration
**AGENT-03 aligned.** Bridge MCP servers to the bus:
- Connect to local MCP servers (STDIO transport)
- Expose MCP tools as bus-native RPC calls
- Agents can't tell the difference between bus tools and MCP tools

**Why it's a differentiator:** MCP is becoming a standard. Being able to use any MCP server as a tool source, distributed across the bus, is powerful.

#### Workflow Scheduling
**AGENT-06 aligned.** Orchestrate multi-agent:
- Sequential: run steps A → B → C
- Parallel: run A, B, C simultaneously, collect results
- Conditional: branch based on output
- Timeout and retry per step

**Why it's a differentiator:** Beyond single-agent tool use — actual workflow orchestration where multiple agents coordinate.

#### Session Persistence
**AGENT-07 aligned.** Survive restarts:
- Serialize agent state (messages, context, pending tasks)
- Restore from disk/file
- Continue conversation from where it left off

**Why it's a differentiator:** Most frameworks lose all state on restart. Persistence enables long-running agent workflows.

### Anti-Features (Deliberately NOT Building)

| Anti-Feature | Why Excluded |
|---|---|
| Built-in vector database | Over-engineered for v1; users have their own DB preferences |
| UI/CLI tooling | Out of scope; API/library first |
| Multi-modal (vision, audio) | Stick to text-first; add later |
| Agent memory beyond session | Use external DB; in-memory + file persistence is enough for v1 |
| Python bindings | Separate effort; BrickOS has Phase 4 for this |
| Built-in auth/authz | Defer to BrickOS Phase 3 |

## Feature Complexity Matrix

| Feature | Complexity | Dependencies | Notes |
|---------|-----------|--------------|-------|
| Agent struct + loop | Medium | LLM client trait | Core building block |
| Tool trait + registry | Medium | LLM client, serialization | Needs provider schema translation |
| LLM client abstraction | High | HTTP client, streaming | Most complex piece — provider differences |
| A2A protocol | High | Bus, task state machine | Distributed coordination |
| Skills system | Medium | Config loader, template engine | Mostly config design |
| MCP integration | Medium | Process spawning, JSON-RPC | Protocol is well-defined |
| Scheduling | Medium | Tokio concurrency | Workflow graph design |
| Streaming | Medium | Tokio streams, SSE parsing | Provider-specific nuances |
| Session persistence | Low | Serialization, file I/O | Straightforward serialize/deserialize |
| Config-driven agents | Low | Config crate | Already exists in workspace |

## Feature Dependencies

```
Agent Struct (AGENT-01)
├── LLM Client Trait
│   └── Streaming (AGENT-08)
├── Tool System (AGENT-02)
│   └── MCP Integration (AGENT-03)
└── Skills System (AGENT-05)

A2A Protocol (AGENT-04)
├── Agent Struct
└── Bus communication

Scheduler (AGENT-06)
├── Agent Struct
└── A2A Protocol

Session (AGENT-07)
└── Serialization

Config (AGENT-09)
├── Agent Struct
├── Skills System
└── Tool Registry
```

**Build order implication:** LLM Client → Agent Struct → Tool Registry → then parallelize MCP, Skills, A2A, Scheduling, Streaming, Session.

---
*Features research for: Distributed LLM Agent Orchestration Framework*
*Researched: 2026-03-19*

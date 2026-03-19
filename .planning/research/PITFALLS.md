# Pitfalls Research

**Domain:** Distributed LLM Agent Orchestration Framework (Rust/Zenoh)
**Researched:** 2026-03-19
**Confidence:** MEDIUM-HIGH

## Critical Pitfalls

### Pitfall 1: Mega-Prompts — Cramming Everything Into Agent System Prompts

**What goes wrong:**
LLMs don't execute 500 instructions reliably. They compress, reinterpret, and skip critical directives. When all agent capabilities, tools, skills, and behaviors are packed into a single system prompt, the agent produces inconsistent, unpredictable behavior in production that wasn't visible in demos.

**Why it happens:**
Developers treat the system prompt as a configuration file rather than a communication medium. The "works in demo, fails in production" pattern emerges because demos use short, focused prompts while production systems accumulate more and more instructions over time.

**How to avoid:**
- Keep system prompts under 2000 tokens. Break complex behaviors into modular prompt templates.
- Use the Skills system (AGENT-05) to compose capabilities from config — each skill gets its own prompt fragment, not one monolithic prompt.
- Implement prompt version management with A/B testing capability.
- Add explicit instruction prioritization (most important rules first, as LLM attention degrades toward end of context).

**Warning signs:**
- Agent behavior changes after adding new tools or skills
- Prompt length exceeds 3000 tokens
- Different LLM providers produce drastically different results with same prompt
- "Works in my test but not in production" reports from users

**Phase to address:** AGENT-01 (Agent struct), AGENT-05 (Skills system) — address in Phase 1-2 during agent/skill design

---

### Pitfall 2: Tight Coupling to Specific LLM Providers

**What goes wrong:**
Hardcoding OpenAI API assumptions into the agent core. When switching to Anthropic, Ollama, or other OpenAI-compatible providers, the agent breaks or produces degraded results. Tool calling schemas, response parsing, and streaming behavior differ across providers but are handled with "just works" assumptions.

**Why it happens:**
The constraint "No external AI SDK crates" is good, but leads some developers to implement only OpenAI-compatible API calls without abstracting provider differences. Many assume all providers implement function calling identically.

**How to avoid:**
- Create a trait-based `LlmClient` abstraction from day one:
  ```rust
  trait LlmClient: Send + Sync {
      async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError>;
      async fn stream_complete(&self, request: LlmRequest) -> Result<StreamingResponse, LlmError>;
      fn supports_function_calling(&self) -> bool;
      fn tool_choice_style(&self) -> ToolChoiceStyle;
  }
  ```
- Abstract tool calling schema generation — each LLM has different JSON Schema requirements
- Handle provider-specific quirks (Anthropic uses different tool format than OpenAI)
- Test with at least 2-3 different providers during development

**Warning signs:**
- Code has hardcoded "gpt-4" or "claude-3" strings
- Tool definitions are written in OpenAI format only
- No abstraction between API call and response parsing
- Streaming code assumes SSE format without error handling

**Phase to address:** AGENT-01 — Phase 1: Build the LLM client abstraction first, not after implementing agent logic

---

### Pitfall 3: Poor Streaming Implementation — Blocking or Silent Failures

**What goes wrong:**
Streaming responses are either not implemented, block the entire bus during generation, or silently fail when the underlying provider doesn't support true streaming. Users experience 8+ second delays waiting for complete responses instead of sub-400ms perceived latency.

**Why it happens:**
- Function call mode often blocks during tool argument generation (vLLM issue)
- SSE handling in Rust is complex; many opt for simpler batch responses
- Some LLM gateways (LiteLLM) fall back to "fake streaming" silently
- Distributed bus integration complicates stream forwarding — each token needs to go through Zenoh

**How to avoid:**
- Implement true streaming from the start; don't default to batch and "upgrade later"
- Use Tokio async streams for SSE handling over the bus
- Handle the specific case of tool calling blocking — some models don't stream tool arguments
- Add proper backpressure: don't let token-by-token bus messages overwhelm subscribers
- Implement connection recovery: streaming can break midstream, handle reconnection gracefully

**Warning signs:**
- No `StreamingResponse` type in the codebase
- LLM client returns `Result<String>` instead of streaming type
- No timeout on streaming requests
- Bus messages flood without flow control

**Phase to address:** AGENT-08 (Streaming responses) — Phase 2: Define streaming interfaces early, implement after LLM client abstraction

---

### Pitfall 4: Tool Calling Schema Mismatches

**What goes wrong:**
LLMs generate JSON that fails validation — missing required fields, wrong types, or schema drift. The tool call fails, the agent loops endlessly, or worse — the agent silently works around the validation failure in unpredictable ways. This is the #1 source of "it works 80% of the time" failures.

**Why it happens:**
- LLMs are next-token predictors, not schema-aware compilers
- Each LLM provider has different JSON Schema subset support
- Tool schemas evolve but cached schemas in agents become stale
- No validation feedback loop — validation failures aren't fed back to the LLM for correction

**How to avoid:**
- Use strict, minimal JSON schemas — avoid complex nested structures in tool definitions
- Add schema validation with recovery:
  - Catch validation failures
  - Attempt JSON repair (common fix patterns)
  - If unrepairable, return clear error to agent with specific field issues
- Use provider-specific schema translation (OpenAI vs Anthropic tool formats differ significantly)
- Test tool schemas with 100+ random inputs before production
- Consider constrained generation libraries if supported by your target providers

**Warning signs:**
- Frequent tool call retries in logs
- Schema evolution without version management
- Tool definitions copied from one provider to another without translation
- No error message parsing from validation failures

**Phase to address:** AGENT-02 (Tool trait + registry) — Phase 1: Build robust schema validation and error recovery into tool system

---

### Pitfall 5: MCP Integration — Local-Only Mindset Breaking at Scale

**What goes wrong:**
MCP starts as STDIO local server and works beautifully for single-user desktop AI. Then teams try to deploy at scale or share tools across agents, and everything breaks: authentication doesn't work, discovery fails, and the "NxM problem" MCP claimed to solve re-emerges because the protocol wasn't designed for distributed agent-to-agent communication.

**Why it happens:**
- MCP was designed for single-machine, single-user scenarios
- Transport layer (STDIO) doesn't generalize to network/distributed scenarios
- Security model is minimal (no built-in auth for remote scenarios)
- Each MCP server is a point-to-point connection, not a many-to-many bus

**How to avoid:**
- Use MCP as one tool source among many, not the only integration point
- Wrap MCP clients in the same trait as bus-based RPC tools — agents shouldn't know the difference
- Implement a local MCP proxy (AGENT-03) that bridges to the distributed bus
- Plan for authentication from the start (MCP has no built-in auth for remote)
- Don't assume MCP servers are discoverable — implement explicit registration

**Warning signs:**
- MCP server list grows without discovery mechanism
- Each agent creates its own MCP connections (resource exhaustion)
- No authentication story for remote MCP servers
- Using STDIO transport in production

**Phase to address:** AGENT-03 (MCP client integration) — Phase 2: Design MCP as one integration path, not the primary one

---

### Pitfall 6: A2A Coordination — Ignoring Task State and Failure Modes

**What goes wrong:**
A2A protocol enables agent-to-agent communication, but most implementations ignore the hard parts: What happens when the delegating agent times out? How do you handle partial results? What about circular delegations? Without proper state management, multi-agent workflows become non-deterministic and unrecoverable.

**Why it happens:**
- A2A protocol is new (late 2024/2025) and still evolving
- Most examples show happy paths, not failure handling
- Task lifecycle management (submission → working → completed/failed) is complex
- Distributed systems introduce network partition issues that single-agent systems don't have

**How to avoid:**
- Implement full A2A task state machine:
  - `submitted` → `working` → `completed` | `failed` | `input-required`
  - Track task IDs for every cross-agent call
  - Handle state transitions atomically (use Zenoh consistency primitives)
- Add timeout with explicit failure states, not indefinite waiting
- Implement task cancellation — agents must be able to stop subtasks
- Handle "input-required" — some tasks need human or external input to continue
- Build retry logic with exponential backoff for transient failures

**Warning signs:**
- No task ID tracking between agents
- Blocking calls that never return
- No timeout on A2A requests
- Silent failures where one agent stops responding but others continue

**Phase to address:** AGENT-04 (A2A protocol), AGENT-06 (Scheduler) — Phase 2: Design task lifecycle first, protocol implementation second

---

### Pitfall 7: Skill Composition — Configuration-Driven Without Boundaries

**What goes wrong:**
Skills are loaded from config (good), but without clear boundaries or conflict resolution, skills interfere with each other. Two skills define a "summarize" tool with different behaviors. System prompts from skills contradict each other. The agent becomes unpredictable as more skills are added.

**Why it happens:**
- Config-driven means no compile-time checks for conflicts
- Skills can override each other's tools silently
- Prompt template composition isn't validated
- No skill versioning or dependency management

**How to avoid:**
- Implement skill namespaces — tools named `skill_a::summarize` not just `summarize`
- Add skill conflict detection at load time:
  - Warn on duplicate tool names
  - Warn on overlapping prompt instructions
- Define skill dependencies: skill B requires skill A
- Add skill versioning to config schema
- Implement skill composition validation before agent initialization

**Warning signs:**
- Tool name collisions at runtime
- Prompt length grows linearly with skill count (should be bounded)
- No way to disable a specific skill
- Conflicting tool behaviors with no clear resolution

**Phase to address:** AGENT-05 (Skills system) — Phase 1: Design skill isolation and conflict resolution from the start

---

### Pitfall 8: Over-Architecting — Building a Framework Before Understanding the Problem

**What goes wrong:**
The project sets out to build a "general-purpose agent framework" and builds elaborate abstractions, configuration systems, and extensibility points before understanding what actual agents need to do. Result: a flexible but unused core with missing production features.

**Why it happens:**
- "Build it and they will come" mentality with frameworks
- Premature abstraction of "future" use cases
- Not starting with a concrete agent use case
- Too much focus on "how to extend" rather than "what it does"

**How to address:**
- Start with a specific agent use case (not "agents" in general)
- Build the minimum viable agent first, then refactor into reusable components
- Let the bus (Zenoh) do the heavy lifting — don't re-implement discovery/routing
- Use rkyv for serialization because it's required, not because it's "better" (for now)

**Warning signs:**
- Abstraction layers that aren't used by any concrete implementation
- "We can support X later" in design docs
- More configuration options than agent features
- Empty crate modules that were "reserved for future use"

**Phase to address:** All phases — Validate each feature against a real use case before building

---

## Technical Debt Patterns

| Shortcut | Immediate Benefit | Long-term Cost | When Acceptable |
|----------|-------------------|----------------|-----------------|
| Hardcoding LLM provider | Faster initial development | Provider switch requires complete rewrite | Never — build abstraction upfront |
| Skipping streaming | Simpler code | Poor UX, users leave | Only for non-interactive batch agents |
| Single tool namespace | Simpler config | Tool conflicts as skills grow | Only for single-skill agents |
| No task state tracking | Simpler A2A implementation | Undebuggable failures in production | Only for fire-and-forget, non-critical tasks |
| Sync LLM calls | Easier error handling | Blocks entire agent, no concurrency | Never in distributed system |

---

## Integration Gotchas

| Integration | Common Mistake | Correct Approach |
|-------------|----------------|------------------|
| Zenoh bus | Treating bus as fire-and-forget | Implement acknowledgments for critical messages |
| rkyv serialization | Using derive without version handling | Add versioning to archive format |
| MCP STDIO | Using in production | Bridge to bus-based transport |
| A2A JSON-RPC | Assuming reliable network | Implement idempotency and retries |
| Tool RPC calls | No timeout | Always timeout, return partial results with status |

---

## Performance Traps

| Trap | Symptoms | Prevention | When It Breaks |
|------|----------|------------|----------------|
| Token-by-token bus messages | Network saturation, subscriber overwhelm | Implement message batching (buffer 50-100ms of tokens) | At 100+ concurrent streaming agents |
| Tool call without result caching | Repeated identical API calls | Add result cache with TTL | At high tool call volume |
| A2A task state in memory | Lost state on restart | Persist to durable storage (AGENT-07) | On any agent restart |
| Session state growth | Memory leak over time | Implement session pruning/TTL | After 10k+ messages |

---

## Security Mistakes

| Mistake | Risk | Prevention |
|---------|------|------------|
| MCP server trusts all clients | Malicious tool registration | Validate tool source, add MCP authentication |
| A2A without auth | Unauthorized agent access | Implement token-based A2A auth |
| Tool results without sanitization | Injection attacks | Validate all tool outputs before feeding back to LLM |
| Config loading from untrusted sources | Config injection | Validate config sources, don't load arbitrary TOML |

---

## "Looks Done But Isn't" Checklist

- [ ] **Streaming:** Appears to work (SSE endpoint exists) — verify true streaming, not fake batch
- [ ] **Tool calling:** Appears to work (tool called) — verify schema validation and error recovery
- [ ] **A2A:** Appears to work (agents communicate) — verify task state tracking and timeout handling
- [ ] **Skills:** Appears to work (loaded from config) — verify no tool name conflicts
- [ ] **MCP:** Appears to work (tools available) — verify discovery and auth work at scale
- [ ] **Session:** Appears to work (state saved) — verify durable across agent restarts

---

## Recovery Strategies

| Pitfall | Recovery Cost | Recovery Steps |
|---------|---------------|----------------|
| Schema mismatch causing tool failures | LOW | Add validation with repair, log specific failures |
| A2A task stuck in "working" | MEDIUM | Implement timeout-based recovery, mark as "failed", notify parent |
| Mega-prompt causing unpredictable behavior | MEDIUM | Audit prompt, reduce to 2000 tokens, split into skills |
| Provider lock-in | HIGH | Build abstraction layer, migrate incrementally |
| Skill conflicts | LOW | Add detection, use namespacing |

---

## Pitfall-to-Phase Mapping

| Pitfall | Prevention Phase | Verification |
|---------|------------------|--------------|
| Mega-prompts | AGENT-01, AGENT-05 (Phase 1) | Prompt audit, token counting |
| LLM provider coupling | AGENT-01 (Phase 1) | Test with 2+ providers |
| Poor streaming | AGENT-08 (Phase 2) | User perceived latency < 500ms |
| Schema mismatches | AGENT-02 (Phase 1) | Fuzz test tool calls with 100+ inputs |
| MCP limitations | AGENT-03 (Phase 2) | Scale test with multiple agents |
| A2A state issues | AGENT-04, AGENT-06 (Phase 2) | Test failure scenarios |
| Skill conflicts | AGENT-05 (Phase 1) | Load 10+ skills, verify no conflicts |
| Over-architecting | All (continuous) | Count abstractions vs implementations |

---

## Sources

- Allen Chan: "Common AI Agent Failures: Architecture Over Model" (LinkedIn, 2026)
- Composio: "The 2025 AI Agent Report: Why AI Pilots Fail in Production" (2025)
- Softcery: "Why AI Agents Fail in Production: Six Architecture Patterns" (2025)
- Dmitry Degtyarev: "Everything That Is Wrong with Model Context Protocol" (Medium, 2025)
- CyberArk: "MCP Security Flaws" (2025)
- O'Reilly: "Designing Collaborative Multi-Agent Systems with the A2A Protocol" (2025)
- Micheal Lanham: "Stop Blaming the LLM: JSON Schema Is the Cheapest Fix" (Medium, 2026)
- Codastra: "LLM Function-Calling Pitfalls Nobody Mentions" (2025)
- vLLM Issues: Streaming latency in function call mode (2026)
- GitHub: LiteLLM streaming fallbacks (2026)

---
*Pitfalls research for: Distributed LLM Agent Orchestration Framework*
*Researched: 2026-03-19*
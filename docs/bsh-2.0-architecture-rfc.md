# BSH 2.0 Architecture RFC
## AI-Native Composable Capability Container & Federated Runtime

- **Status:** Proposed
- **Version:** 0.2.0
- **Date:** 2026-11-06
- **Target:** BOS / BSH
- **Scope:** BSH local capability composition, workspace runtime, and multi-node federation
- **Decision baseline:** ADR-001 through ADR-009 (ADR-008 and ADR-009 accepted 2026-11-06)
- **Audience:** BOS/BSH maintainers, Runtime engineers, extension authors, security reviewers

---

## 1. Abstract

BSH 2.0 evolves BSH from an AI Agent Harness into an **AI-native composable capability container** that can assemble purpose-specific AI workspaces and cooperate with other BSH instances through a secure federation.

A BSH instance provides a governed runtime in which capabilities can be discovered, selected, composed, authorized, activated, executed, observed, and released. Capabilities may be implemented natively, provided by Ferris extensions, exposed through MCP, hosted in WASM or external processes, or reached remotely through another BSH node.

Multiple BSH instances form a **BSH Federation**. Each node remains an autonomous runtime and an independent security boundary. Nodes can expose explicitly granted capabilities, delegate tasks, collaborate through agents, and share selected resources or knowledge without merging their local authority or state.

The design has two complementary composition scopes:

1. **Local composition:** assemble local and authorized remote capabilities into a Workspace.
2. **Federated composition:** coordinate capabilities, agents, tasks, and resources across multiple autonomous BSH nodes.

The architecture is deliberately hybrid: declarative configuration establishes the permitted boundary; AI may propose task-specific compositions within that boundary; deterministic runtime components validate, authorize, and execute the resulting plan.

This RFC defines the target architecture, domain model, protocol boundaries, security requirements, migration approach, implementation phases, and acceptance criteria. It does not claim that the proposed components are already implemented.

## 2. Motivation

### 2.1 Current product direction

BSH should not become a monolithic desktop Agent with an ever-growing fixed tool list. Different tasks require different capability sets, context, permissions, runtime resources, and collaboration patterns.

Examples include:

- Software engineering: source control, code navigation, terminal, tests, issue tracking, and review.
- Research: web discovery, browser, papers, citation, knowledge retrieval, and synthesis.
- Data analysis: files, SQL, Python, statistics, visualization, and reporting.
- Operations: observability, deployment interfaces, diagnostics, and controlled remediation.
- General assistance: selected personal or work capabilities under narrowly scoped permissions.

A fixed global registry exposed to every Agent is inefficient and difficult to govern. BSH needs an explicit composition layer that turns a task or profile into a bounded execution environment.

### 2.2 Multi-node requirement

A single BSH instance cannot always provide every tool, operating environment, model, dataset, or compute resource. Multiple instances should be able to cooperate:

- A desktop BSH can coordinate work while a Linux BSH runs isolated builds.
- A node with a browser capability can expose a narrowly scoped remote browser service.
- A compute node can offer an authorized inference or analysis capability.
- Multiple Agents can divide research, implementation, and verification.
- A node can query approved knowledge sources without exporting their underlying data.

This is not merely remote procedure calling. Federation must address identity, trust, discovery, authorization, delegation, task state, cancellation, failure recovery, and audit.

### 2.3 Design goals

BSH 2.0 MUST:

1. Treat capabilities as first-class runtime objects.
2. Support declarative and AI-assisted dynamic composition.
3. Instantiate isolated, reproducible Workspaces.
4. Keep the Composition Engine independent of any specific Agent implementation.
5. Support multiple capability provider types behind stable contracts.
6. Support autonomous BSH nodes and secure federation.
7. Keep local and remote authorization boundaries explicit.
8. Make composition plans explainable, inspectable, and reproducible.
9. Preserve existing BOS, Ferris, Built-in Tools, MCP, and SDK investments where practical.
10. Allow multiple interfaces to use the same underlying Runtime.

### 2.4 Non-goals

This RFC does not require:

- A single global Agent or centralized execution server.
- A global shared filesystem or shared credential store.
- Automatic trust between nodes merely because they belong to one Federation.
- Unrestricted capability forwarding or transitive privilege delegation.
- A single transport technology for all deployments.
- Replacing Ferris, MCP, or the existing Agent/ReAct engine.
- Implementing every federation feature in the first release.
- Treating AI-generated plans as trusted executable instructions.

## 3. Terminology

| Term | Definition |
|---|---|
| **BSH instance** | One running BSH application/runtime installation. |
| **Node** | The federation identity and protocol representation of a BSH instance. A process may host one or more explicitly configured node identities, but identity must not be inferred from a display name. |
| **Capability** | A versioned, described unit of functionality that can be discovered and invoked under policy. |
| **Capability Provider** | The implementation or adapter that makes a Capability available. |
| **Composition** | A declarative, validated specification of capabilities and their relationships for a purpose. |
| **Workspace** | An instantiated, policy-bounded runtime environment derived from a Composition. |
| **Execution Context** | The bounded identity, inputs, context, policy, and cancellation scope for an execution. |
| **Federation** | A trust and policy domain connecting independently operated BSH Nodes. |
| **Peer** | A remote Node with an explicit relationship and trust state. |
| **Remote Capability** | A local proxy/descriptor for a capability exposed by a remote Node; it is not a copy of the remote implementation. |
| **Delegation** | A bounded request for another Node or Agent to perform a task or subtask. |
| **Collaboration** | A coordinated interaction among multiple Agents or Nodes toward a shared task. |
| **Resource Lease** | A time-bounded grant to use a remote resource under defined constraints. |
| **Composition Snapshot** | An immutable record of the resolved composition, versions, policies, and configuration used for a Workspace instance. |

## 4. Architecture principles

### 4.1 Capability First

Capabilities are the common abstraction for local and remote functionality. Tools, Skills, Services, Resources, Agents, and extension-contributed surfaces are not forced into one implementation type, but they participate in a common discovery and governance model.

### 4.2 Composition over monolith

New functionality SHOULD be delivered as a capability provider or composable module rather than by continually expanding the core Runtime.

### 4.3 Runtime first, interface independent

Desktop, CLI, API, MCP-facing interfaces, and language SDKs MUST use the same core contracts. UI state must not become the authority for capability availability or authorization.

### 4.4 AI proposes; Runtime disposes

AI may recommend or request a composition. Deterministic components MUST validate dependencies, compatibility, policy, resource bounds, and authorization before activation or execution.

### 4.5 Independent security boundaries

Every Node retains its own identity, policy engine, secrets, filesystem boundary, execution isolation, and audit records. Federation membership does not imply shared authority.

### 4.6 Explicit, least-privilege trust

Trust is established through authenticated identities and explicit grants. A peer receives only the capabilities and scopes that have been granted to it.

### 4.7 Reproducibility and explainability

A composition must be inspectable and serializable. A running Workspace must be traceable to a Composition Snapshot and policy versions.

### 4.8 Transport agnostic

Federation semantics MUST NOT depend on Zenoh, QUIC, HTTP/2, WebSocket, or another transport. Transport adapters carry protocol messages; they do not define authorization semantics.

ADR-009 selects Zenoh as the first transport implementation. The principle is unchanged: the protocol is carried by an adapter, and another adapter may be added without changing identity, authorization, invocation, delegation, or task semantics.

## 5. System architecture

### 5.1 Logical architecture

```text
                         BSH Interfaces
                Desktop / CLI / API / SDK / MCP
                               |
                         Workspace API
                               |
                    Workspace Management
                               |
                       Composition Engine
                  /            |             \
             Discovery       Resolver       Validator
                  \            |             /
                      Capability Graph
                               |
                     Capability Runtime
             /          /       |        \          \
         Built-in    Ferris    MCP     WASM/Process  Remote
          Provider  Provider Adapter  Providers    Provider
                               |
                        Execution Engine
                               |
                         Security Kernel
             Identity / Policy / Sandbox / Audit
                               |
                         Core Runtime
            Event Bus / State / Context / Storage
                               |
                    Federation Subsystem
       Identity / Trust / Discovery / Protocol / Delegation
              /                    |                    \
         BSH Node A             BSH Node B             BSH Node C
```

This diagram describes logical responsibilities, not mandatory crate boundaries or a runtime call sequence.

### 5.2 Planes

The architecture is divided into related but separately governed planes.

#### Local capability plane

Owns capability descriptors, providers, local discovery, composition, Workspace activation, and local execution.

#### Federation control plane

Owns Node identity, membership, peer trust, discovery metadata, federation policy, and revocation.

#### Federation coordination plane

Owns delegation, collaboration, scheduling metadata, task state exchange, cancellation, and recovery coordination.

#### Federation data plane

Carries authorized remote capability invocations, task inputs/results, selected events, and resource traffic.

A control-plane coordinator may assist discovery or scheduling, but MUST NOT become an implicit authority to execute on member nodes.

### 5.3 Hybrid topology

BSH Federation uses a hybrid topology:

- Nodes can communicate directly when connectivity and policy permit.
- An optional coordinator can assist membership, rendezvous, discovery, or scheduling.
- Nodes remain operational when a coordinator is unavailable, subject to the loss of coordinator-dependent functions.
- Federation membership and trust are explicit and revocable.
- Multiple federations may connect through separately approved trust relationships.

A coordinator is not a mandatory central execution server.

## 6. Core domain model

The six local composition concepts are extended with federation concepts.

### 6.1 Capability

A Capability is a versioned contract describing an operation or service.

A descriptor SHOULD include:

- Stable capability ID and semantic version.
- Human-readable name and description.
- Capability kind and operation identifiers.
- Input/output schema and content types.
- Required permissions and resource scopes.
- Side-effect classification and risk metadata.
- Provider identity and provider version.
- Availability and lifecycle state.
- Compatibility requirements.
- Invocation constraints: timeout, payload limits, concurrency, and idempotency where applicable.
- Whether streaming, cancellation, or progress reporting is supported.

Capability metadata is untrusted input until validated. Descriptions may help an LLM select a capability, but they MUST NOT grant authority.

### 6.2 Capability Provider

A Provider implements one or more capabilities. Initial provider categories include:

- Native Rust/Built-in provider.
- Ferris extension provider.
- MCP adapter provider.
- WASM component provider.
- External process provider.
- Remote BSH provider.
- Future language or transport adapters.

Provider lifecycle, package validation, and runtime isolation remain the responsibility of the appropriate host/runtime layer.

### 6.3 Composition

A Composition is a declarative specification of the capabilities and constraints needed for a purpose.

It SHOULD contain:

- Composition ID, version, and human-readable purpose.
- Capability references and version constraints.
- Provider constraints where relevant.
- Dependency relationships and activation order constraints.
- Optional/required capability declarations.
- Configuration references (not plaintext secrets).
- Context Profile reference.
- Permission Profile reference.
- Resource budgets and execution constraints.
- Allowed remote Nodes/Federations, if any.
- Conflict and substitution policy.
- Composition validation requirements.

Composition is a desired-state specification. It is not itself proof that every referenced capability is available or authorized.

### 6.4 Workspace

A Workspace is an instantiated runtime environment derived from a validated Composition.

A Workspace owns or references:

- Resolved capability set and provider instances.
- Workspace-scoped tool view exposed to an Agent.
- Context Profile and bounded context resources.
- Permission Profile and execution scopes.
- Runtime configuration and resource budgets.
- Session/state namespace and persistence policy.
- Lifecycle state and cancellation scope.
- Composition Snapshot.
- Local and remote capability bindings.

Workspaces MUST be isolated according to their configured security boundary. A Workspace is not merely a UI tab or a conversation.

### 6.5 Execution Context

Execution Context binds a specific invocation to:

- Principal and authenticated identity.
- Workspace and task identifiers.
- Capability and operation.
- Input and resource scope.
- Effective policy decision.
- Deadline, cancellation token, and budget.
- Trace and audit correlation identifiers.
- Delegation provenance where applicable.

### 6.6 Runtime

Runtime coordinates lifecycle and execution. It provides stable services for:

- Capability registration and lookup.
- Composition validation and resolution.
- Workspace instantiation and teardown.
- Authorization and policy enforcement.
- Provider invocation and cancellation.
- Context and state access.
- Event publication and observability.
- Snapshot and audit persistence.

The Composition Engine MUST remain independent of a specific Agent or ReAct implementation.

## 7. Capability taxonomy and compatibility

Capability kinds describe semantics, not necessarily separate registries.

| Kind | Purpose | Examples |
|---|---|---|
| Tool | Atomic invocation | Read file, search repository, HTTP request |
| Skill | Reusable method or task procedure | Code review, literature review |
| Service | Long-lived endpoint or process | LSP, database service |
| Resource | Addressable data/context | Document, dataset, repository |
| Agent | Delegated decision-making principal | Research Agent, verifier Agent |
| Workflow | Declarative multi-step composition | Build and test pipeline |

A Skill may reference Tools, Services, Resources, or other Skills. An Agent may receive a Workspace-scoped view of capabilities. A Workflow may invoke capabilities through the same authorization and execution contracts.

The model SHOULD avoid treating every Skill or Agent as a simple synchronous Tool. Long-running and interactive capabilities need lifecycle, progress, cancellation, and result contracts.

## 8. Local composition lifecycle

### 8.1 Composition pipeline

1. Receive a task, profile, or explicit Composition.
2. Discover candidate capabilities from approved registries.
3. Filter candidates by declared allowlists, version constraints, availability, and policy.
4. Resolve dependencies and provider compatibility.
5. Build a dependency graph and detect cycles/conflicts.
6. Calculate the effective permission and resource requirements.
7. Validate the proposed plan against policy.
8. Present an explainable plan for approval where required.
9. Create a Composition Snapshot.
10. Instantiate providers and Workspace resources.
11. Expose a Workspace-scoped capability view to the Agent/interface.
12. Execute under per-invocation authorization.
13. Release or retain resources according to lifecycle policy.

### 8.2 Static and dynamic composition

BSH uses a hybrid model.

**Declarative/static composition** defines the maximum permitted capability and policy boundary. It is appropriate for repeatable, managed, and auditable environments.

**Dynamic composition** allows AI or an application to select and activate capabilities within that boundary.

Dynamic composition MUST NOT:

- Expand the Workspace's maximum authority.
- silently install or execute an untrusted provider.
- bypass dependency, version, or compatibility validation.
- turn an unapproved remote peer into a trusted peer.
- persist secrets in a Composition or Snapshot.

### 8.3 Composition Snapshot

A Snapshot SHOULD record:

- Composition ID and revision.
- Resolved capability IDs and exact versions.
- Provider IDs, versions, and runtime types.
- Dependency graph and resolution decisions.
- Configuration references and non-secret effective settings.
- Context and permission profile versions.
- Policy decision references.
- Local/remote origin and peer identity for remote capabilities.
- Runtime compatibility metadata.
- Activation result and failure diagnostics.

Snapshots MUST NOT contain plaintext credentials, private keys, or secret values. Reproducibility is bounded by external dependencies and mutable remote services; the Snapshot must identify those dependencies and their version/identity where possible.

## 9. Workspace lifecycle and isolation

A Workspace SHOULD support the following conceptual states:

```text
Draft -> Validating -> Ready -> Starting -> Active
                         |                   |
                         v                   v
                       Failed <---------- Degraded
                                             |
                              +--------------+-------------+
                              |              |             |
                            Paused         Stopping      Failed
                              |              |
                            Active         Stopped
```

The exact state machine and transitions must be finalized during implementation design. Invalid transitions MUST be rejected.

Workspace operations include:

- Create from Composition.
- Validate without activation.
- Start/activate.
- Pause/resume where supported.
- Update through a new validated revision.
- Cancel in-flight work.
- Stop and release resources.
- Snapshot and inspect.
- Destroy according to retention policy.

Isolation must cover more than UI or registry visibility. It includes filesystem scope, process environment, credentials, network access, memory/state namespace, resource budgets, and provider lifecycle as applicable.

## 10. AI-native capability discovery and assembly

### 10.1 Discovery

The Discovery Engine can use:

- Structured metadata filters.
- Capability tags and categories.
- Semantic retrieval over descriptions and schemas.
- Task/context relevance.
- Availability and latency signals.
- Policy eligibility.
- Provider trust and compatibility.

Semantic relevance is not authorization. A candidate may be relevant but unavailable or prohibited.

### 10.2 AI composition proposal

The AI-facing interface should return a structured proposal, not executable code. A proposal SHOULD include:

- User/task objective.
- Selected capabilities and rationale.
- Required dependencies.
- Expected side effects.
- Permission requirements.
- Remote Nodes involved.
- Estimated resource requirements.
- Alternatives and uncertainty.
- Approval requirements.

The deterministic Composition Engine validates the proposal and returns a normalized plan with explicit allow/deny/approval outcomes.

### 10.3 Tool surface optimization

Agents should receive a Workspace-scoped, task-relevant tool view rather than the entire global capability catalog. This reduces prompt overhead, ambiguity, and accidental tool selection.

The capability catalog remains available to the Discovery Engine; it need not be injected wholesale into every model context.

### 10.4 Explainability

For each selected or rejected capability, the system SHOULD be able to explain:

- Why it was selected or excluded.
- Which policy and dependency constraints applied.
- Whether the capability is local or remote.
- What permissions are required.
- What will happen if it is activated.

## 11. BSH Federation

### 11.1 Federation goals

Federation enables independent BSH Nodes to discover and use explicitly exposed capabilities, delegate tasks, collaborate, and share selected resources.

Federation does not merge runtimes. Every Node remains authoritative for its own resources and execution decisions.

### 11.2 Node identity

Each Node MUST have a stable cryptographic identity distinct from its display name, hostname, IP address, or transport endpoint.

The identity model SHOULD support:

- Key/certificate rotation.
- Revocation.
- Multiple transport endpoints.
- Node metadata with integrity protection.
- Explicit enrollment and trust establishment.
- Audit correlation across identity changes.

The concrete key management and enrollment mechanism is a security design decision to be validated before production rollout.

### 11.3 Peer relationship

A Peer record SHOULD include:

- Node identity and verified public identity material.
- Configured endpoints and supported protocol versions.
- Trust state and enrollment provenance.
- Granted capabilities and scopes.
- Federation membership.
- Expiration/renewal metadata.
- Last successful authenticated contact.
- Revocation status.

Discovery MUST NOT automatically imply trust or permission.

### 11.4 Federation membership

A Federation is a policy and trust domain, not a shared superuser domain.

Membership SHOULD define:

- Federation ID and policy revision.
- Member Node identities.
- Enrollment and removal rules.
- Permitted discovery visibility.
- Allowed collaboration/delegation modes.
- Cross-federation trust rules.
- Revocation and incident procedures.

### 11.5 Topology

The target is hybrid P2P:

- Direct peer-to-peer communication where possible.
- Optional coordinator for rendezvous, membership distribution, discovery, or scheduling assistance.
- No mandatory centralized execution authority.
- Store-and-forward or relay behavior may be added where required, with explicit confidentiality and availability trade-offs.

The initial implementation should favor a small, testable topology rather than a general-purpose distributed control plane.

## 12. Remote Capability model

### 12.1 Remote Capability proxy

A Remote Capability is a local representation of a remote capability contract. It contains the remote Node identity, capability reference/version, endpoint binding, local grant reference, and invocation constraints.

It MUST NOT be treated as a locally implemented capability. Invocation must cross an authenticated federation boundary and be authorized by both sides.

### 12.2 Invocation contract

A remote invocation SHOULD carry:

- Protocol version and message ID.
- Caller Node and authenticated principal.
- Target Node, capability ID, and operation.
- Workspace/task/correlation IDs.
- Input payload or authorized resource reference.
- Deadline and cancellation semantics.
- Idempotency key where supported.
- Delegation provenance.
- Trace context.

Responses SHOULD distinguish:

- Success.
- Validation failure.
- Authentication/authorization denial.
- Capability unavailable.
- Rate/resource limit.
- Timeout.
- Cancellation.
- Remote execution failure.
- Transport failure.
- Unknown outcome.

A transport failure after a request was accepted may leave an unknown execution outcome. Clients MUST NOT assume that retrying is safe unless the operation is idempotent or protected by a deduplication key.

### 12.3 Dual authorization

The caller Node checks whether the local principal/Workspace may invoke the remote capability. The provider Node independently checks whether the authenticated caller is allowed to invoke the operation with the requested scope.

Neither decision substitutes for the other.

### 12.4 Remote capability lifecycle

Remote capability availability should account for:

- Peer connectivity.
- Remote provider state.
- Grant validity.
- Capability version compatibility.
- Health/lease expiration.
- Rate and concurrency limits.
- Revocation.

A stale discovery record is not proof of availability. Invocation-time checks remain necessary.

## 13. Task delegation and Agent collaboration

### 13.1 Delegation

Delegation transfers a bounded task request, not unrestricted control of the delegating Node.

A Delegation SHOULD define:

- Delegation ID and parent task ID.
- Requesting Node and principal.
- Target Node or Agent.
- Task objective and bounded input.
- Allowed capability/resource scope.
- Deadline and resource budget.
- Data handling constraints.
- Result contract and acceptance criteria.
- Cancellation and retry policy.
- Whether sub-delegation is prohibited or explicitly permitted.

### 13.2 Delegation chain

Every delegated operation must preserve provenance. The target Node must know the authenticated immediate caller and the relevant original delegation context.

A delegate MUST NOT gain more authority than the grant explicitly allows. By default, sub-delegation is disabled. If enabled, the grant must define maximum depth, allowed recipients, scope attenuation, and audit requirements.

### 13.3 Collaboration

Collaboration may include:

- Parallel task execution.
- Sequential handoff.
- Independent verification.
- Consensus or review workflows.
- Shared task status and bounded artifact exchange.

The protocol should not mandate a particular multi-agent reasoning strategy. Coordination policy belongs above the transport and capability invocation layers.

### 13.4 Task state

Long-running tasks need durable state transitions, progress, cancellation, and recovery. Suggested states:

```text
Created -> Accepted -> Running -> Succeeded
                    |     |       |
                    |     +-----> Failed
                    |             |
                    +----------> Rejected
                          Running -> CancelRequested -> Cancelled
```

The final state machine must define races among completion, cancellation, timeout, and peer loss.

### 13.5 Failure semantics

Distributed execution cannot assume a global transaction. The system SHOULD use:

- Idempotency keys and deduplication.
- Durable task records.
- Explicit deadlines and leases.
- Bounded retries with backoff.
- Compensation where side effects cannot be rolled back.
- Unknown-outcome reporting.
- Reconciliation after reconnection.
- Explicit ownership of task state.

Exactly-once execution across arbitrary nodes is not promised. The target is well-defined at-least-once or at-most-once behavior per operation contract, with deduplication where supported.

## 14. Resource and knowledge federation

### 14.1 Resource federation

Nodes may expose selected resources such as compute slots, model inference endpoints, browser sessions, datasets, or storage operations.

Resource access SHOULD use leases with:

- Resource identity/type.
- Owner Node.
- Granted operations and limits.
- Expiration and renewal.
- Quota/concurrency.
- Revocation behavior.
- Accounting and audit.

A Resource Lease does not grant access to the host outside the explicitly exposed resource interface.

### 14.2 Knowledge federation

Knowledge federation should prefer query delegation and controlled result exchange over bulk replication.

Policies must define:

- Which indexes or collections are discoverable.
- Which queries are allowed.
- Whether results may be cached.
- Data classification and redaction.
- Retention and deletion behavior.
- Whether source references or provenance must be returned.

The default should be data minimization: do not export raw source data when an authorized query result is sufficient.

## 15. Federation protocol

### 15.1 Protocol boundary

Federation Protocol is a transport-independent application protocol. It defines identity binding, message envelope, version negotiation, request/response semantics, errors, task state, cancellation, and authorization context.

ADR-009 selects Zenoh as the first transport implementation, including for resource streams. The protocol remains defined above it: another transport adapter may be added without changing identity, authorization, invocation, delegation, or task semantics.

### 15.2 Message envelope

A protocol envelope SHOULD include:

- Protocol name and version.
- Message ID.
- Message type.
- Sender and recipient Node identities.
- Issued-at and expiry/deadline.
- Correlation and trace IDs.
- Payload schema/version.
- Authentication/integrity binding.
- Optional idempotency key.

The envelope serialization is JSON, with an explicit protocol name and schema version so a newer sender cannot silently widen what an older receiver accepts (ADR-009). Canonical signing rules still require a separate protocol specification and interoperability tests.

### 15.3 Protocol capabilities

Initial protocol feature negotiation should cover:

- Identity and enrollment version.
- Capability discovery.
- Remote invocation.
- Task delegation and status.
- Cancellation.
- Progress/events.
- Resource leases.
- Optional streaming.
- Optional knowledge query.

Unsupported features must fail explicitly rather than silently degrading security semantics.

### 15.4 Transport responsibilities

A transport adapter is responsible for connectivity, framing, delivery characteristics, and transport-level security integration. It MUST NOT independently decide capability authorization or alter the meaning of protocol messages.

Under the Zenoh implementation (ADR-009), transport access control is defense in depth, and link-level TLS authenticates a hop rather than a principal. The envelope signature and the receiving Node's Security Kernel are what bind authority.

### 15.5 Event delivery

Events are not a substitute for durable task state. Important state transitions must be recoverable through an authoritative task/status API or durable log. Event delivery may be duplicated, delayed, or lost depending on the transport.

## 16. Trust and security model

### 16.1 Threat model

The design must account for:

- Malicious or compromised peers.
- Stolen or rotated credentials.
- Forged discovery metadata.
- Malicious capability descriptions or schemas.
- Prompt injection through remote results.
- Replay and duplicate requests.
- Confused-deputy and privilege escalation.
- Unauthorized sub-delegation.
- Data exfiltration through tools or artifacts.
- Denial of service and resource exhaustion.
- Coordinator compromise or unavailability.
- Partial network partitions and stale state.

### 16.2 Required controls

| Control | Requirement |
|---|---|
| Node identity | Cryptographically authenticated, revocable identity |
| Mutual authentication | Authenticate both communicating Nodes |
| Capability grants | Explicit operation and resource scope |
| Least privilege | Default deny; grant only required scope |
| Dual authorization | Caller-side and provider-side decisions |
| Delegation provenance | Preserve original and immediate principals |
| Token lifetime | Short-lived credentials or bounded grants |
| Replay protection | Nonce/message freshness and deduplication as appropriate |
| Isolation | Provider-specific sandbox/process boundaries |
| Secret handling | No plaintext secrets in manifests, snapshots, or logs |
| Audit | Record allowed and denied decisions on each relevant Node |
| Revocation | Revoke Node, peer, grant, capability, and lease |
| Resource limits | Bound time, memory, output, concurrency, and payload |
| Fail closed | Authentication/policy uncertainty denies sensitive operations |

### 16.3 Authorization model

Authorization should evaluate at least:

- Subject: Node, principal, Agent, Workspace.
- Action: capability operation.
- Resource: path, domain, dataset, service, or other scoped target.
- Context: task, time, risk, policy, and delegation provenance.

A local Workspace grant is not automatically a remote grant. Remote invocation requires an explicit federation grant, and the provider Node enforces its own policy.

### 16.4 Delegation attenuation

Delegated authority must be equal to or narrower than the authority granted by the delegator and provider. Sub-delegation is denied by default. No peer may manufacture authority by forwarding a broader token or omitting provenance.

### 16.5 Prompt injection boundary

Remote capability descriptions, tool outputs, documents, events, and Agent messages are untrusted data. They may inform reasoning but cannot alter system policy, grant permissions, or execute instructions outside the authorized contract.

### 16.6 Audit and observability

Each Node maintains its own authoritative audit record. Correlation IDs allow cross-node investigation but do not require a shared mutable audit database.

Audit events SHOULD include identity, capability, operation, scope decision, policy version, task/delegation ID, outcome, timing, and relevant resource accounting. Sensitive input/output data must be redacted or excluded according to policy.

## 17. Relationship to existing BOS components

This RFC is an evolution plan, not a replacement mandate.

| Existing component | Target responsibility |
|---|---|
| BOS Core Runtime | Shared runtime foundation |
| `agent` / ReAct | Agent reasoning and execution principal |
| Agent ToolRegistry | Agent-facing dispatch view; not the global capability authority |
| ReAct ToolRegistry | Engine-internal dispatch/cancellation implementation |
| Built-in Tools | Native Capability Provider |
| Ferris | Extension/provider framework and lifecycle |
| MCP | External protocol adapter/provider |
| WASM / Process runtimes | Isolated provider execution options |
| BOS Bus / Zenoh | Event and transport substrate; the selected federation transport (ADR-009), while the protocol definition stays independent of it |
| Skills | Reusable task methods and capability compositions |
| GPUI / BSH UI | Workspace and federation management interface |
| Rust/Python/JavaScript SDKs | Stable integration surfaces |

### 17.1 Registry boundaries

The architecture should distinguish:

1. **Capability Registry:** metadata, identity, versions, provider references, and availability.
2. **Workspace Capability View:** the authorized and resolved subset visible to one Workspace/Agent.
3. **Execution Dispatch Registry:** the runtime-specific mapping used to invoke a provider.

These may share implementation internally, but their contracts and authority must remain distinct. Existing Agent and ReAct registries should not be removed merely to create a new name; migration must preserve their legitimate layer-specific roles.

### 17.2 Ferris integration

Ferris remains responsible for extension package and runtime concerns such as manifest validation, dependency handling, lifecycle, runtime selection, and sandbox integration.

BSH should adapt Ferris-contributed surfaces into the common Capability model. The Capability model must not duplicate Ferris package management or weaken its security checks.

### 17.3 MCP integration

MCP is an adapter/protocol integration, not the canonical BSH capability model. MCP tools/resources/prompts may be mapped into BSH capability descriptors while retaining source identity, transport behavior, and limitations.

### 17.4 Zenoh integration

Zenoh is the selected federation transport (ADR-009): it already sits below the BOS stack as the bus substrate, and it carries both protocol messages and chunked resource streams over one endpoint per Node. Federation Protocol must remain transport-independent, and its identity, authorization, delegation, and task semantics must not be delegated to Zenoh: a Zenoh id is an endpoint, not a Node identity, and Zenoh access control is never the authorization decision.

The intra-Node bus codec (rkyv) is not used between Nodes; the federation envelope is JSON with an explicit protocol and schema version.

## 18. Proposed subsystem boundaries

The following are logical boundaries. Exact crate names and dependency directions must be decided after source-level Gap Analysis.

| Subsystem | Responsibility |
|---|---|
| Capability Domain | Capability/provider descriptors, IDs, versions, schemas |
| Capability Registry | Registration, lookup, metadata indexing |
| Discovery Engine | Structured and semantic discovery |
| Composition Engine | Resolve dependencies, conflicts, compatibility, and plans |
| Workspace Manager | Workspace lifecycle, isolation, state |
| Capability Runtime | Provider activation and invocation |
| Security Kernel | Identity, authorization, policy, sandbox coordination, audit |
| Context Manager | Context profiles, retrieval, budgets, provenance |
| Federation Core | Node identity, membership, peer/trust state |
| Federation Protocol | Message envelope, negotiation, request/task semantics |
| Federation Transport | Pluggable transport adapters; Zenoh is the first implementation (ADR-009) |
| Delegation Manager | Remote task lifecycle and provenance |
| Collaboration Manager | Multi-Agent coordination abstractions |
| Resource Manager | Remote resource leases and quotas |
| Observability | Local and cross-node correlation, metrics, traces |

### 18.1 Dependency constraints

- Composition Engine must not depend on Agent/ReAct.
- Capability Domain must not depend on GPUI.
- Federation Protocol must not depend on a specific transport.
- Security decisions must not be delegated to UI or LLM output.
- Provider implementations must not bypass the Security Kernel.
- Remote Capability invocation must pass through the same local Workspace authorization view and remote provider authorization.
- Federation code must not directly access another Node's private internal registries or storage.

## 19. API design direction

The following Rust-like signatures are illustrative contracts, not existing APIs. They are intended to guide later interface design and must be reconciled with current BOS conventions before implementation.

```rust
pub struct CapabilityId(String);
pub struct ProviderId(String);
pub struct CompositionId(String);
pub struct WorkspaceId(String);
pub struct NodeId(String);
pub struct FederationId(String);
pub struct DelegationId(String);

pub trait CapabilityCatalog {
    async fn discover(
        &self,
        query: CapabilityQuery,
        scope: DiscoveryScope,
    ) -> Result<Vec<CapabilityDescriptor>, CapabilityError>;
}

pub trait CompositionEngine {
    async fn validate(
        &self,
        composition: CompositionSpec,
        context: ValidationContext,
    ) -> Result<ValidatedComposition, CompositionError>;

    async fn instantiate(
        &self,
        composition: ValidatedComposition,
        context: WorkspaceContext,
    ) -> Result<WorkspaceHandle, WorkspaceError>;
}

pub trait WorkspaceManager {
    async fn inspect(&self, id: &WorkspaceId) -> Result<WorkspaceSnapshot, WorkspaceError>;
    async fn stop(&self, id: &WorkspaceId) -> Result<(), WorkspaceError>;
}

pub trait FederationTransport: Send + Sync {
    async fn connect(&self, peer: PeerAddress) -> Result<PeerSession, FederationError>;
    /// Sends one sealed frame and returns whether it left this Node, so a
    /// caller can tell a retry-safe failure from an unknown outcome. The
    /// protocol seals a message into a frame before it reaches a transport, so
    /// a transport cannot alter or fabricate one.
    async fn send(
        &self,
        peer: &NodeId,
        frame: Vec<u8>,
    ) -> Result<DeliveryState, FederationError>;
    async fn close(&self, peer: &NodeId) -> Result<(), FederationError>;
}

/// Byte streams are message sequences keyed by lease, not extra trait methods.
pub enum ResourceStreamMessage {
    Open { lease: LeaseId, uri: String, offset: u64 },
    Chunk { lease: LeaseId, seq: u64, bytes: Vec<u8> },
    Credit { lease: LeaseId, bytes: u64 },
    Close { lease: LeaseId, total: u64, checksum: [u8; 32] },
}

pub trait FederationService {
    async fn discover_capabilities(
        &self,
        peer: &NodeId,
        query: CapabilityQuery,
    ) -> Result<Vec<RemoteCapabilityDescriptor>, FederationError>;

    async fn invoke_remote(
        &self,
        request: RemoteInvocation,
    ) -> Result<RemoteInvocationResult, FederationError>;

    async fn delegate(
        &self,
        request: DelegationRequest,
    ) -> Result<DelegationHandle, FederationError>;
}
```

API requirements:

- Async operations must have bounded deadlines and cancellation semantics.
- Errors must distinguish policy denial from transport and provider failures.
- A send must report whether the message left the Node (`DeliveryState`), so retry safety is decidable rather than assumed.
- IDs must be stable and validated; display names are not identifiers.
- Schema evolution must be versioned.
- Long-running operations need handles and status APIs.
- API callers must not be able to construct trusted authorization decisions.

## 20. Configuration and manifest direction

A Composition manifest should be declarative and should reference secrets rather than embed them.

Illustrative YAML:

```yaml
apiVersion: bsh.open1s.dev/v1
kind: Composition

metadata:
  id: software-engineering
  version: 1.0.0
  description: Workspace for repository development and verification

spec:
  capabilities:
    - id: filesystem.read
      version: "^1.0"
      required: true
    - id: git.repository
      version: "^1.0"
      required: true
    - id: test.runner
      version: "^1.0"
      required: true
    - id: github.issues
      version: "^1.0"
      required: false

  contextProfile: engineering
  permissionProfile: engineering-limited

  federation:
    mode: allowlisted
    peers:
      - node-id: "node:linux-builder"
        capabilities:
          - build.execute
          - test.execute

  resources:
    maxConcurrentInvocations: 8
    defaultTimeoutSeconds: 120

  approval:
    requireFor:
      - network.write
      - repository.push
      - deployment.execute
```

This example is conceptual. The final schema must define strict parsing, unknown-field behavior, canonical IDs, version constraints, secret references, and compatibility guarantees.

## 21. Implementation roadmap

Implementation should proceed incrementally. Each phase must preserve working behavior and introduce testable contracts.

### Phase 0 — Architecture and source audit

Deliverables:

- This RFC reviewed and accepted.
- Current source-level Gap Analysis.
- Existing crate/API inventory.
- Capability and Workspace domain mapping.
- Threat model and security review.
- Migration and compatibility plan.

Exit criteria:

- No unresolved ownership conflict between existing registries and proposed Capability Registry.
- Dependency direction and security enforcement points are documented.
- Proposed contracts map to concrete current source locations or explicitly identified gaps.

### Phase 1 — Capability foundation

Deliverables:

- Capability IDs/descriptors and version model.
- Provider contract and adapter boundary.
- Registry metadata and lookup.
- Workspace-scoped capability view.
- Built-in and Ferris integration proof.

Exit criteria:

- Native and extension capabilities can be discovered through one stable contract.
- Agent-facing tool view contains only Workspace-authorized capabilities.
- Existing tool execution behavior remains covered by regression tests.

### Phase 2 — Composition Engine and Workspace Runtime

Deliverables:

- Manifest parser and validator.
- Dependency resolver and conflict detection.
- Static composition.
- Dynamic proposal validation.
- Composition Snapshot.
- Workspace lifecycle and isolation integration.

Exit criteria:

- A Workspace can be created reproducibly from a validated Composition.
- Invalid dependencies, policy violations, and incompatible versions fail before activation.
- Dynamic composition cannot expand the declared permission boundary.
- Lifecycle and cancellation tests pass.

### Phase 3 — Federation identity, trust, and discovery

Deliverables:

- Node identity and enrollment design.
- Peer/trust state.
- Federation membership and revocation.
- Protocol envelope and version negotiation.
- Transport adapter prototype.
- Capability discovery across authenticated peers.

Exit criteria:

- Unknown peers are not trusted by discovery alone.
- Revoked peers cannot establish new authorized sessions.
- Transport failure does not bypass identity or authorization.
- Protocol conformance tests cover supported message versions.

### Phase 4 — Remote Capability and delegation

Deliverables:

- Remote Capability proxy.
- Dual authorization.
- Remote invocation and structured errors.
- Idempotency/deduplication contract.
- Delegation lifecycle, cancellation, and audit provenance.

Exit criteria:

- Remote calls are authorized by both caller and provider Nodes.
- Expired/revoked grants are rejected.
- Retry behavior is safe or explicitly reports unknown outcome.
- Delegation cannot increase privilege.

### Phase 5 — Multi-Agent collaboration and resource federation

Deliverables:

- Task status/progress API.
- Collaboration abstractions.
- Parallel and sequential delegation patterns.
- Resource leases and quotas.
- Failure recovery and reconciliation.

Exit criteria:

- Node loss and reconnection do not corrupt authoritative task state.
- Cancellation and completion races are tested.
- Resource leases expire and revoke correctly.
- Cross-node audit correlation is possible.

### Phase 6 — Knowledge federation and advanced composition

Deliverables:

- Policy-controlled remote knowledge query.
- Provenance and data minimization.
- AI-assisted federated composition.
- Composition optimization and explainability.
- Federation administration and diagnostics.

Exit criteria:

- Remote knowledge access is scoped, auditable, and governed by retention rules.
- AI cannot infer permission from retrieved content.
- Cross-federation access requires explicit trust and grants.

## 22. Testing and verification strategy

### 22.1 Unit tests

- Descriptor parsing and validation.
- ID/version compatibility.
- Dependency resolution and cycle detection.
- Composition conflict handling.
- Policy evaluation and scope matching.
- Workspace state transitions.
- Delegation state transitions.
- Message envelope validation.

### 22.2 Integration tests

- Built-in provider registration and invocation.
- Ferris provider integration.
- MCP adapter mapping.
- Static and dynamic Workspace composition.
- Local/remote capability invocation.
- Dual authorization.
- Cancellation and timeout propagation.
- Snapshot creation and inspection.
- Peer enrollment and revocation.
- Chunked resource stream over the federation transport, including credit, resume, and lease ownership.

### 22.3 Federation fault tests

- Peer offline before invocation.
- Peer disconnects after accepting a request.
- Duplicate message delivery.
- Delayed and out-of-order events.
- Coordinator unavailable.
- Credential expiration and rotation.
- Revocation during an active task.
- Partial network partition and reconnection.
- Remote provider crash.
- Resource lease expiration.

### 22.4 Security tests

- Unauthorized capability and scope.
- Confused-deputy attempts.
- Delegation privilege escalation.
- Unauthorized sub-delegation.
- Replay and duplicate request handling.
- Malformed or oversized payloads.
- Prompt injection through remote metadata/results.
- Secret leakage in logs and snapshots.
- Sandbox escape regression coverage.
- Fail-closed behavior on policy/identity uncertainty.

### 22.5 Compatibility tests

- Existing Agent and ReAct tool behavior.
- Existing Ferris extension lifecycle and manifests.
- Existing MCP integrations.
- Rust/Python/JavaScript public SDK contracts where applicable.
- Desktop and headless runtime operation.

## 23. Operational requirements

Production federation requires:

- Node health and peer connectivity visibility.
- Capability availability and latency metrics.
- Task/delegation status and failure reason visibility.
- Policy decision and audit inspection.
- Credential rotation and revocation operations.
- Rate limits and per-peer quotas.
- Bounded queues and payload sizes.
- Backpressure and concurrency control.
- Version compatibility reporting.
- Safe shutdown and in-flight task handling.

A node must be able to operate locally when federation connectivity is unavailable. Remote-dependent Workspace capabilities should become unavailable or degraded explicitly; they must not silently fall back to a broader local capability.

## 24. Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Capability model becomes too generic | Weak contracts and complex adapters | Keep semantic capability kinds and strict schemas |
| Duplicate registries proliferate | Inconsistent availability and authorization | Separate authority from dispatch views; define one metadata authority |
| AI dynamically over-composes | Cost, latency, attack surface | Allowlisted boundaries, budgets, deterministic validation |
| Federation becomes centralized | Availability and trust bottleneck | Hybrid P2P, optional coordinator, node autonomy |
| Remote capability is treated as local | Confused-deputy and policy bypass | Explicit Remote Capability proxy and dual authorization |
| Delegation escalates privilege | Cross-node compromise | Attenuated grants, provenance, no sub-delegation by default |
| Distributed retries duplicate side effects | Data corruption or repeated actions | Idempotency keys, deduplication, explicit unknown outcomes |
| Event bus is mistaken for durable state | Lost task transitions | Durable task state and reconciliation APIs |
| Transport lock-in | Difficult future interoperability | Transport-independent protocol |
| Single transport implementation | Availability and migration tied to Zenoh | Protocol above the transport (ADR-005, ADR-009); a second adapter is permitted without semantic change |
| Scope expands too quickly | Delayed delivery and fragile system | Phase gates and minimal federation MVP |

## 25. Architecture Decision Records

The following decisions form the current baseline. ADR-004 through ADR-007 extend the previously selected ADR-001 through ADR-003. ADR-008 and ADR-009 were accepted on 2026-11-06 and bind the federation work. Section 30 maps each decision to its implementation and tests.

### ADR-001 — Composition strategy

**Decision: Hybrid.**

Use declarative/static definitions to establish the permitted boundary and dynamic composition to select capabilities within that boundary.

### ADR-002 — Composition Engine ownership

**Decision: Independent.**

Composition Engine is a Runtime subsystem independent of Agent/ReAct. Agents may request compositions but do not own composition authority.

### ADR-003 — Capability implementation and execution

**Decision: Hybrid.**

Support multiple provider and execution models behind common capability contracts. Do not force all providers into one implementation technology.

### ADR-004 — Federation topology

**Decision: Hybrid P2P.**

Support direct peer communication and optional coordination services. No mandatory central execution authority.

### ADR-005 — Federation protocol

**Decision: Transport agnostic.**

Define application-level identity, authorization context, invocation, delegation, and task semantics independently of transport adapters.

### ADR-006 — Node autonomy and security boundary

**Decision: Independent Runtime and independent security boundary.**

Every Node owns its identity, policies, secrets, resources, execution, and authoritative audit records. Federation membership does not merge authority.

### ADR-007 — Remote composition

**Decision: Local Composition plus explicitly authorized Remote Capabilities.**

A Workspace may include local and remote capabilities, but remote capabilities remain identifiable as remote and are subject to caller-side and provider-side authorization.

### ADR-008 — Composition layers

**Decision: two composition layers over one Capability Model.**

Local Composition assembles capabilities inside one Workspace: one Security Kernel, one lifecycle, one authoritative state. Federation Composition is a plan over autonomous Nodes — grants, delegation, leases, and knowledge queries — with no shared lifecycle and no shared authority.

The layers share capability IDs, descriptors, the invocation contract, and the error taxonomy. They do not share authority, state ownership, lifecycle, isolation, or transport endpoints. A remote capability is a binding marked as remote (ADR-007), never a local provider, and no Workspace spans Nodes. The collaboration modes — capability, agent, task, resource, and knowledge federation — are composed from two primitives: invoke a capability, and delegate a task.

### ADR-009 — Federation transport, wire format, and resource streams

**Decision: Zenoh, JSON, one endpoint.**

- Zenoh carries the federation protocol and the resource streams; a Node has one federation endpoint. ADR-005 is unchanged: a second transport adapter may be added without changing identity, authorization, or task semantics.
- The envelope is JSON with an explicit protocol name and schema version. The intra-Node bus codec stays rkyv; it is not used between Nodes because its archived layout is build-specific.
- The resource path has no separate federation transport. Its remote half implements the stream protocol — open, chunk, credit, close — directly on `FederationTransport`, as message sequences rather than additional trait methods.
- Node identity is an ed25519 key bound to the Zenoh id by a signed enrollment record. Zenoh access control is defense in depth and link TLS is hop-by-hop; the envelope signature and the receiving Node's kernel decide.
- Discovery publishes only public metadata: name, version, kind, and risk class. Everything else requires a grant.

Consequences: one endpoint and one identity surface per Node; the QUIC data plane in the resource subsystem leaves the federation path at parity; JSON payload size is bounded by invocation limits, with binary content base64 or chunked.

## 26. Open questions

The following items require design validation and should not be silently assumed. Items 2, 3, and 7 are resolved by ADR-009; the rest remain open.

1. What is the canonical Node identity and enrollment mechanism: self-managed certificates, an optional trust authority, or both?
   **Resolved for v1 in code:** an Ed25519 seed whose public key is the NodeId, provisioned on
   first use and enrolled explicitly (section 30.6). A trust authority remains open.
2. ~~Which transport should be implemented first for the federation MVP?~~ **Resolved by ADR-009:** Zenoh, including resource streams. The protocol remains transport-independent.
3. ~~What serialization and schema evolution strategy should Federation Protocol use?~~ **Resolved by ADR-009:** JSON envelope with an explicit protocol and schema version. Canonical signing rules remain open.
4. Which operations require human approval by default, and how are approval policies expressed?
5. What is the initial Workspace isolation boundary across macOS, Linux, and Windows?
6. How should long-running remote task state be persisted and reconciled?
7. ~~Which capability metadata is safe to publish to federation peers?~~ **Resolved for v1 by ADR-009:** public metadata is name, version, kind, and risk class; everything else requires a grant.
8. How should remote artifacts be transferred, referenced, retained, and deleted?
9. Which federation functions are required for the first usable release versus later phases?
10. Which existing crate should own each logical subsystem after source-level dependency analysis?

## 27. Proposed MVP

The first federation MVP should be deliberately narrow:

1. Two BSH Nodes with independently provisioned identities.
2. Explicit peer enrollment and revocation.
3. Authenticated capability discovery.
4. One remote capability invocation path over Zenoh (ADR-009).
5. Caller-side and provider-side authorization.
6. Bounded request/response with timeout and structured errors.
7. Audit records on both Nodes.
8. A Workspace that composes one local and one remote capability.
9. Integration tests for denial, peer loss, timeout, revocation, and duplicate requests.

Do not include general multi-Agent consensus, cross-federation trust, distributed scheduling, or knowledge replication in the MVP. Add task delegation after the remote capability contract and identity/security foundation are proven.

## 28. Acceptance criteria for BSH 2.0 architecture

The architecture is considered implemented only when the following are demonstrated.
The current status of each criterion is recorded in section 30.

### Local composition

- A Composition can be declared, validated, resolved, and instantiated.
- A Workspace exposes only its authorized capability set.
- Built-in, Ferris, and MCP-backed capabilities can be represented through the common model.
- Composition Snapshots capture resolved versions and policy references.
- AI-generated proposals cannot bypass deterministic validation or expand permissions.

### Federation

- Nodes have authenticated, revocable identities.
- Peers require explicit trust and grants.
- Remote capabilities remain distinct from local providers.
- Remote calls require authorization at both ends.
- Delegation preserves provenance and cannot increase authority.
- Task and invocation failures have explicit, testable semantics.
- Each Node retains independent execution and audit authority.

### Engineering quality

- Public contracts are versioned and documented.
- Unit, integration, fault-injection, and security tests cover the critical boundaries.
- Existing BOS/BSH behavior remains regression-tested.
- Cross-platform claims are made only for platforms with passing implementation and CI evidence.
- Documentation reflects actual implementation; proposed functionality is clearly labeled until delivered.

## 29. Conclusion

BSH 2.0 should become more than an Agent Harness or a desktop shell. It should be a **Distributed AI-Native Composable Capability Platform**.

At the node level, BSH assembles purpose-specific Workspaces from governed capabilities. At the federation level, multiple autonomous BSH Nodes discover authorized capabilities, delegate bounded tasks, collaborate through Agents, and share selected resources or knowledge.

The core architectural contract is:

- **Capability** defines what can be done.
- **Provider** defines how it is implemented.
- **Composition** defines what is assembled.
- **Workspace** defines where and under which boundaries it runs.
- **Execution Context** defines who is acting and under what constraints.
- **Runtime** validates, governs, and executes.
- **Federation** connects autonomous Nodes without merging their authority.

The strategic direction is therefore:

**One Runtime, Composable Workspaces, Federated Capabilities, Independent Trust Boundaries.**

This RFC establishes the target architecture. The source-level review is recorded in [BSH 2.0 Composable & Federation Gap Analysis](bsh-2.0-composable-federation-gap-analysis.md), audited against the repository revision linked in that document. That review refines the implementation-status summary below: a domain primitive is not considered a complete runtime feature unless lifecycle, ownership, budgets, persistence, recovery, and fault behavior are also evidenced. Implementation should proceed phase by phase, with build, regression, fault-injection, and security evidence at every gate.

## 30. Implementation status (2026-11-06)

This section maps ADR-001 through ADR-009 and the section 28 acceptance criteria to
the code and tests that exist today. A bullet is marked implemented only when a
test pins it. Gaps are named rather than implied.

### 30.1 Decisions

| ADR | Decision | Implementation | Tests | Remaining gap |
| --- | --- | --- | --- | --- |
| 001 | Composition strategy: hybrid static boundary + dynamic selection | CompositionSpec, proposal gate, resolver, CompositionManifest/lock, AssemblyEngine | capability composition/proposal/lock/assembly tests | The model call itself is host-owned; no automatic LLM invocation ships |
| 002 | Composition Engine ownership: independent of Agent/ReAct | crates/capability + Agent provider adapters | capability and agent tests | Domain ownership is independent; complete host-neutral orchestration/activation boundary remains to be completed |
| 003 | Execution: hybrid provider implementations behind one contract | capability::provider, WorkspaceRuntime, WorkspaceLifecycle, NativeToolProvider | capability/tests/workspace, lifecycle; agent native_provider | Lifecycle, budget enforcement, workspace ownership and activation journal are implemented; provider-kind coverage remains adapter-specific |
| 004 | Topology: autonomous Nodes, independent kernels | federation::service, PeerStore per Node, no shared workspace | federation/tests/service, trust | none known |
| 005 | Protocol: signed JSON envelope, explicit protocol and schema | federation::envelope | federation/tests (envelope, service, signing_vectors) | canonical form now specified and pinned (docs/federation-signing.md); no second implementation yet |
| 006 | Autonomy: dual authorization, denials name the side | service::invoke, serve; CapabilityError::Denied | service tests (caller/provider refusals) | none known |
| 007 | Remote composition: remote is a binding, never a local provider | composition::Binding::Remote, RemoteCapabilityProvider, bsh attach_remote_tools | federation remote_provider, bsh caps attach test | no local fallback by construction |
| 008 | Two layers, one Capability Model, separate authority | one CompositionSpec/resolve; SnapshotOrigin; workspace id per Node; knowledge contract; durable delegation/lease stores | capability composition, snapshot, bsh compose, knowledge and knowledge_wire | Knowledge is opt-in: a host must provide a KnowledgeService and authorizer; broader collaboration orchestration remains GAP-FED-005 |
| 009 | Zenoh, JSON, one endpoint, resource streams on the protocol | federation::zenoh, service resource path; bus::TlsConfig for transport TLS | zenoh_transport, resource, resource_wire, bus TLS tests | TLS settings are exposed, rendered, and validated by Zenoh, and a TLS listener without usable material fails instead of opening plaintext; a mutual-TLS two-session test now performs a real handshake with CA-signed server and client certificates and sends a sealed frame. Production security profile still incomplete; see GAP-FED-003. The ingress queue is bounded (1024 frames, 256 announcements) with drop-and-log overflow, the peer store is written 0600, and accepted delegations persist to delegations.json across restarts; a structured result_contract executes on the target's runtime (scope-checked, cancel-aware) and settles the record, delegation status/cancel are owner-only, handler dispatch is admitted at 64 global / 8 per peer with a retryable refusal, invocations carry an absolute deadline plus a scoped, idempotent remote cancel, resource streams are peer-bound with ordered, bounded delivery, discovery records expire after five minutes with clock-skew and replay rejection, inbound frames are deduplicated by sender-scoped message/idempotency keys, envelopes are recipient- and result-peer-bound, and resource checksums are incremental |

### 30.2 Local composition (section 28)

| Criterion | Status | Evidence or gap |
| --- | --- | --- |
| Declare, validate, resolve, instantiate | implemented | capability/tests/composition.rs, workspace.rs |
| Workspace exposes only its resolved set | implemented foundation | LocalScope + resolver tests; dispatch context ownership hardening remains outstanding (GAP-COMP-004) |
| Built-in, Ferris, and MCP capabilities in one model | implemented | built-in tools declare metadata; MCP tools declare network/medium/Unknown; extension (Ferris-runtime) tools declare medium/extension/Unknown and now enter the composition through the live overlay, which `merged_registry` folds into the plan and snapshot |
| Snapshots capture versions and policy refs | implemented | capability/tests/snapshot.rs, bsh compositions.rs |
| AI proposals cannot bypass validation | implemented as a gate | capability::proposal checks the host's approved ids, a risk ceiling, and remote policy, then returns an ApprovedComposition token that resolve_approved requires; the public resolver is named resolve_host, so no neutral resolve remains to call by accident (a compile_fail doctest pins that the token has no other constructor); tests pin that it refuses a spec the plain resolver would accept. The producer now exists in agent::composition: prompt() renders the catalogue, parse() strictly parses the model JSON, and compose() runs parse -> approve -> resolve_approved with no variant that skips the gate. The host still supplies the model call; no automatic LLM invocation ships |

### 30.3 Federation (section 28)

| Criterion | Status | Evidence or gap |
| --- | --- | --- |
| Authenticated, revocable identities | implemented | NodeIdentity, peers.json, CLI revoke |
| Durable delegation and lease state | implemented | delegations.json and leases.json are atomic 0600, startup recovery marks interrupted delegations Failed and drops expired leases; restart tests |
| Knowledge federation | implemented (opt-in) | knowledge contract/service, signed knowledge_query/publication kinds, provenance, class/purpose/scope authorization, retention/revocation, wire tests |
| Invocation context propagation | implemented | task, trace and delegation hops cross the envelope; a probe provider test pins them |
| Bounded handler admission | implemented | independent invocation/control/resource budgets with retryable refusals and saturation tests |
| Signed key rotation | implemented | RotationRecord signed by the old key; PeerStore::rotate moves grants and revokes the old id; bsh federation rotate/accept-rotation; tests |
| Mutual TLS transport | implemented (cross-process) | tests/tls_transport.rs opens two Zenoh sessions with CA-signed server/client certificates and hostname verification, then sends a sealed frame; a second test runs the listener in a separate OS process |
| Durable collaboration graph | implemented foundation | versioned TaskGraph, deterministic ready order, bounded executor, persisted transitions, restart recovery, failure containment |
| Explicit trust and grants | implemented | PeerStore, enrollment tests, CLI |
| Remote distinct from local providers | implemented | RemoteCapabilityProvider; compose skips a remote id a local tool owns |
| Authorization at both ends | implemented | service tests, bsh end-to-end test |
| Delegation preserves provenance, cannot widen authority | implemented | delegation tests (attenuation, parent subset), service audit |
| Explicit, testable failure semantics | implemented | error taxonomy, DeliveryState, UnknownOutcome tests, offline test |
| Independent execution and audit authority | implemented | per-Node kernel, per-process audit files |

### 30.4 Engineering quality (section 28)

| Criterion | Status | Evidence or gap |
| --- | --- | --- |
| Public contracts versioned and documented | implemented | descriptor versions, VersionSource; rustdoc gate |
| Unit, integration, fault-injection, security tests | implemented for the boundaries named above | forged/expired/schema frames, revocation, quota, torn-log tests |
| Existing behavior regression-tested | implemented | workspace-wide test suites; bsh settings/shell tests |
| Cross-platform claims only with CI evidence | not claimed | no CI evidence recorded for this work; macOS-only runs so far |
| Documentation reflects implementation | implemented | this appendix, bsh-federation.md, adapter research |

### 30.5 MVP checklist (section 27)

| # | Item | Status |
| --- | --- | --- |
| 1 | Two Nodes with independent identities | implemented (two in-process Nodes in tests) |
| 2 | Explicit enrollment and revocation | implemented |
| 3 | Authenticated discovery | implemented |
| 4 | One remote capability path over Zenoh | implemented (Zenoh transport; in-memory transport in tests) |
| 5 | Caller-side and provider-side authorization | implemented |
| 6 | Bounded request/response, timeout, structured errors | implemented |
| 7 | Audit records on both Nodes | implemented |
| 8 | Workspace with one local and one remote capability | implemented |
| 9 | Tests for denial, peer loss, timeout, revocation, duplicates | implemented (duplicate handling via idempotency key; no retry) |

### 30.6 Open questions (section 26) status

- Item 1 (canonical identity): resolved for v1 in code - an Ed25519 seed whose public
  key is the NodeId, provisioned on first use, enrolled explicitly. A trust authority
  is still open.
- Item 4 (human approval): not decided; the capability registry switch is opt-in, and
  no approval policy is attached to remote invocation.
- Item 5 (cross-platform isolation): unproven; all evidence is macOS.
- Item 6 (long-running task persistence): delegation and lease stores persist and
  recover explicitly; broader task-graph orchestration remains GAP-FED-005.
- Item 8 (artifact retention): leases bound bytes, but no retention or deletion policy.
- Item 10 (crate ownership): settled by the source-level analysis in
  bsh-2.0-adapter-research.md.


### 30.7 Verification run (2026-11-06, second pass)

The first pass ran under a restricted file sandbox: the outer policy denied
the nested sandbox-exec the shell tests need (`sandbox_apply: Operation not
permitted`), so agent::tools::bash and the caps heel/network cases failed. The
same suites were rerun under an unconfined policy, where every one of them
passes:

| Suite | Result |
| --- | --- |
| capability (unit tests + doctests) | 31 passed |
| federation | 106 passed |
| agent | 570 passed, 0 failed |
| bsh caps::tests | 146 passed, 1 ignored |
| bsh extension::e2e | 15 passed |
| bsh settings:: | 145 passed |
| bus | 15 passed |
| bsh right-panel federation UI + helpers | 13 passed |
| bsh federation directory lock | 2 passed |
| bsh federation node start/stop + discovery + exposure | 11 node tests passed |
| bsh prompts:: | 7 passed |
| bsh store:: | 20 passed |
| bsh mcp:: | 21 passed |
| bsh federation_node / federation_audit / federation CLI / compositions / export | 27 passed |
| clippy (capability, federation, agent, bsh; all targets, -D warnings) | clean |
| rustdoc (same four crates, -D warnings) | clean |
| cargo fmt --check (same four crates) | clean |

The two composition tests that the live-overlay change regressed - the plan
and the snapshot counted only the base registry, not the live overlay - were
found by the first pass and fixed: the turn's surface is the base registry plus
the live overlay.

The whole bsh library suite was not run to completion inside the verification
budget; the module runs above cover every module this work touched, and the
remaining modules are unrelated to it.



### 30.8 Source-level gap review

The detailed, code-referenced review is maintained separately to keep this RFC readable:

- [BSH 2.0 Composable & Federation Gap Analysis](bsh-2.0-composable-federation-gap-analysis.md)

The review identifies the following implementation blockers beyond the domain-model foundation:

- Reproducible composition artifact, registry snapshot and lockfile.
- Budget propagation and enforcement at the Workspace dispatch boundary.
- Transactional Workspace lifecycle and resource ownership.
- Cross-Workspace invocation-context validation.
- Dependency graph and deterministic activation order.
- Complete AI Assembly pipeline beyond proposal validation.
- Durable delegation/lease state and restart recovery.
- Bounded federation ingress and a production transport security profile.
- First-class Knowledge Federation and higher-level collaboration orchestration.

These are not claims that the existing primitives are defective in isolation. They are the missing end-to-end guarantees needed before the corresponding BSH 2.0 product capabilities can be described as complete.

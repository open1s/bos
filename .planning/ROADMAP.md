# BrickOS Roadmap

**Project:** BrainOS - Distributed message bus with Zenoh

**Phase 1: RPC on Bus** *(completed)*
Implement RPC (Remote Procedure Call) functionality on top of the existing Zenoh pub/sub bus.

---

## Phase 1: RPC on Bus

**Goal:** Add request/response RPC pattern on top of existing pub/sub

**Deliverables:**
- RPC client wrapper (like QueryWrapper but for RPC semantics)
- RPC server/service wrapper (similar to QueryableWrapper)
- Support for typed requests/responses
- Timeout handling
- Error propagation

**Status:** Planned (2 plans)

Plans:
- [x] 01-01-PLAN.md — Foundation: RpcError, RpcResponse types, RpcClient ✅
- [x] 01-02-PLAN.md — RpcService, RpcDiscovery, integration tests ✅

---

## Phase 2: Service Discovery & Health Monitoring

**Goal:** Service enumeration, health monitoring with heartbeats, liveness checks

**Deliverables:**
- DiscoveryRegistry for listing all advertised services via wildcard subscription
- HealthPublisher for periodic heartbeat publishing
- HealthChecker for querying service liveness
- Extended DiscoveryInfo with version and health_topic fields
- ServiceCache for in-memory TTL-based caching of discovery and health results
- Debug eprintln! cleanup

**Status:** Completed (1 plan)

Plans:
- [x] 02-01-PLAN.md — Discovery registry, health types, service cache, debug cleanup ✅

---

## Future Phases

- Phase 3: Authentication/Authorization
- Phase 4: Python Bindings Completion

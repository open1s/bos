# BrickOS Roadmap

**Project:** BrainOS - Distributed message bus with Zenoh

**Phase 1: RPC on Bus** *(current)*
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

**Status:** Ready for discussion

---

## Future Phases

- Phase 2: Service Discovery
- Phase 3: Authentication/Authorization
- Phase 4: Python Bindings Completion

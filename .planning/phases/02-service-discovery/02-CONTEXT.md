# Phase 02: Service Discovery & Health Monitoring - Context

**Gathered:** 2026-03-19
**Status:** Ready for planning

<domain>
## Phase Boundary

Enhance the existing RpcDiscovery with:
1. **Service enumeration** — List all advertised services via wildcard subscription
2. **Health monitoring** — Heartbeat system + liveness queries
3. **Bug fix** — Remove debug `eprintln!` statements from discovery.rs
</domain>

<existing_implementation>
## What's Already Built (Phase 01)

### RpcDiscovery (`crates/bus/src/rpc/discovery.rs`)
- `RpcDiscovery::announce()` — publishes `DiscoveryInfo` to `rpc/services/{name}` once
- `DiscoveryQueryBuilder::query()` — subscribes to `rpc/services/{name}` and waits for ONE response
- `DiscoveryInfo` — `{ topic_prefix, service_name }`

### Issues with Current Discovery
1. **Debug statements** — Two `eprintln!` lines on lines 137 and 143
2. **Single service only** — `query()` finds ONE specific service, no enumeration
3. **No health tracking** — No heartbeat, no liveness check
4. **DiscoveryInfo too minimal** — No version, metadata, or health endpoint

### How RpcService Announces
- `RpcService::announce()` publishes to `rpc/services/{name}`
- Published once on init, not periodically
- No way to know if service is still alive
</existing_implementation>

<proposed_design>
## Proposed Design

### 1. Service Enumeration

Add `DiscoveryRegistry` for listing all services:

```rust
// New struct in discovery.rs
pub struct DiscoveryRegistry {
    session: Option<Arc<Session>>,
    timeout: Duration,
}

impl DiscoveryRegistry {
    /// Subscribe to `rpc/services/*` and collect all announcements.
    pub async fn list_services(&self) -> Result<Vec<DiscoveryInfo>, ZenohError>;
}
```

### 2. Health Monitoring

New health module with heartbeat system:

```rust
// New: crates/bus/src/rpc/health.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    pub service_name: String,
    pub status: ServiceState,  // Online, Degraded, Offline
    pub version: String,
    pub timestamp: u64,       // Unix timestamp
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServiceState {
    Online,
    Degraded,
    Offline,
}

// Service-side: HealthPublisher
pub struct HealthPublisher {
    service_name: String,
    version: String,
    state: ServiceState,
    interval: Duration,
}

impl HealthPublisher {
    pub fn start(self, session: Arc<Session>) -> JoinHandle<()>;
    pub fn set_state(&mut self, state: ServiceState);
}

// Client-side: HealthChecker
pub struct HealthChecker { ... }

impl HealthChecker {
    /// Check liveness of a specific service.
    pub async fn check(&self, service_name: &str) -> Result<HealthStatus, ZenohError>;
    
    /// Check all known services.
    pub async fn check_all(&self) -> Result<Vec<HealthStatus>, ZenohError>;
}
```

### 3. Extended DiscoveryInfo

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryInfo {
    pub topic_prefix: String,
    pub service_name: String,
    pub version: String,           // NEW
    pub health_topic: String,      // NEW: topic for health status
}
```

### 4. Topic Conventions

- Discovery announcements: `rpc/services/{name}`
- Health heartbeats: `rpc/health/{name}`
- Wildcard subscription: `rpc/services/**` and `rpc/health/**`
</proposed_design>

<canonical_refs>
## Canonical References

- `crates/bus/src/rpc/discovery.rs` — Existing discovery, needs enhancement
- `crates/bus/src/rpc/service.rs` — RpcService, RpcServiceBuilder
- `crates/bus/src/subscriber.rs` — SubscriberWrapper pattern for subscriptions
- `crates/bus/src/publisher.rs` — PublisherWrapper pattern for publishing
- `.planning/codebase/CONVENTIONS.md` — Error handling, builder pattern, async patterns
</canonical_refs>

<deferred>
## Deferred Ideas

- Service caching (per user request)
- Load balancing across instances
- Circuit breaker pattern
- Authentication on discovery/health topics
</deferred>

---

*Phase: 02-service-discovery*
*Context gathered: 2026-03-19*

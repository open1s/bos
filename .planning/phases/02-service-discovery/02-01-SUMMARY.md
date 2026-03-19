# Plan 02-01 Summary: Service Discovery & Health Monitoring

**Phase:** 02-service-discovery
**Plan:** 01
**Status:** ✅ Completed
**Date:** 2026-03-19

---

## What Was Built

Service discovery enumeration + health monitoring with TTL-based caching.

### Core Components

1. **DiscoveryRegistry** (`crates/bus/src/rpc/discovery.rs`)
   - Subscribes to `rpc/services/**` (wildcard) and collects all announcements within a timeout
   - Builder: `.session()`, `.timeout()`, `.list_services()`
   - Deduplicates by `service_name`

2. **HealthPublisher + HealthChecker** (`crates/bus/src/rpc/health.rs`)
   - `HealthPublisher::new()` → `.version()`, `.interval()`, `.start(session)` → JoinHandle
   - `HealthPublisher::set_state()` for dynamic state changes (Online/Degraded/Offline)
   - `HealthChecker::check(name)` → single service health
   - `HealthChecker::check_all()` → all services via `rpc/health/**`
   - Heartbeats published to `rpc/health/{service_name}` at configurable interval

3. **ServiceCache** (`crates/bus/src/rpc/cache.rs`)
   - In-memory TTL cache for `DiscoveryInfo` and `HealthStatus`
   - `tokio::sync::RwLock` + `HashMap` — no external dependencies
   - `put_service()`, `get_service()`, `get_all_services()`, `remove_service()`, `cleanup_services()`
   - `put_health()`, `get_health()`, `cleanup_health()`
   - `stats()` → `CacheStats` with total/expired counts
   - Clone semantics: empty on clone (independent cache instances)

4. **Extended DiscoveryInfo** (`crates/bus/src/rpc/discovery.rs`)
   - Added `version` (default `"1.0.0"`) and `health_topic` (default `"rpc/health/{name}"`)

5. **Debug Cleanup**
   - Removed `eprintln!("DEBUG: ...")` statements from `discovery.rs`

### Files Changed

| File | Changes |
|------|---------|
| `crates/bus/src/rpc/discovery.rs` | DiscoveryRegistry, extended DiscoveryInfo, debug cleanup |
| `crates/bus/src/rpc/health.rs` | **NEW** — HealthPublisher, HealthChecker, HealthStatus, ServiceState |
| `crates/bus/src/rpc/cache.rs` | **NEW** — ServiceCache, CacheStats |
| `crates/bus/src/rpc/mod.rs` | Module exports, integration tests, updated existing test assertions |
| `crates/bus/src/lib.rs` | Public exports for health + cache types |

### New Tests

- Unit tests: 5 health module tests, 7 cache module tests
- Integration tests: `test_discovery_registry_list_services`, `test_health_publisher_checker`
- Updated: `test_rpc_full_cycle` — now verifies extended DiscoveryInfo fields

---

## Verification Results

- `cargo check -p bus` → **PASS** (0 errors)
- `cargo test -p bus` → **52 passed, 0 failed**
- All acceptance criteria from plan tasks verified

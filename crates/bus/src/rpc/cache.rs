//! In-memory TTL cache for discovered services and health status.

use std::collections::HashMap;
use std::fmt::Debug;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

use super::discovery::DiscoveryInfo;
use super::health::HealthStatus;

struct CacheEntry<T> {
    value: T,
    expires_at: Instant,
}

impl<T> CacheEntry<T> {
    fn is_expired(&self) -> bool {
        Instant::now() > self.expires_at
    }
}

impl<T: Debug> Debug for CacheEntry<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CacheEntry")
            .field("value", &self.value)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Service cache for discovered services and health status.
///
/// Stores `DiscoveryInfo` and `HealthStatus` with TTL-based expiration.
/// Thread-safe via `tokio::sync::RwLock`.
///
/// # Example
/// ```rust,ignore
/// let cache = ServiceCache::new(Duration::from_secs(30));
/// cache.put_service(info).await;
/// if let Some(info) = cache.get_service("my-service").await {
///     println!("Found: {:?}", info);
/// }
/// ```
#[derive(Debug)]
pub struct ServiceCache {
    services: RwLock<HashMap<String, CacheEntry<DiscoveryInfo>>>,
    health: RwLock<HashMap<String, CacheEntry<HealthStatus>>>,
    default_ttl: Duration,
}

impl ServiceCache {
    /// Create a new cache with the given default TTL.
    pub fn new(ttl: Duration) -> Self {
        Self {
            services: RwLock::new(HashMap::new()),
            health: RwLock::new(HashMap::new()),
            default_ttl: ttl,
        }
    }

    /// Get the default TTL.
    pub fn default_ttl(&self) -> Duration {
        self.default_ttl
    }

    // --- Service (DiscoveryInfo) cache operations ---

    /// Insert or update a discovered service.
    pub async fn put_service(&self, info: DiscoveryInfo) {
        let entry = CacheEntry {
            value: info,
            expires_at: Instant::now() + self.default_ttl,
        };
        let name = entry.value.service_name.clone();
        let mut services = self.services.write().await;
        services.insert(name, entry);
    }

    /// Get a cached service by name. Returns `None` if expired or missing.
    pub async fn get_service(&self, name: &str) -> Option<DiscoveryInfo> {
        let services = self.services.read().await;
        services.get(name).and_then(|e| {
            if e.is_expired() {
                None
            } else {
                Some(e.value.clone())
            }
        })
    }

    /// Get all non-expired cached services.
    pub async fn get_all_services(&self) -> Vec<DiscoveryInfo> {
        let services = self.services.read().await;
        services
            .values()
            .filter(|e| !e.is_expired())
            .map(|e| e.value.clone())
            .collect()
    }

    /// Remove a cached service.
    pub async fn remove_service(&self, name: &str) {
        let mut services = self.services.write().await;
        services.remove(name);
    }

    /// Clean up expired entries from service cache.
    pub async fn cleanup_services(&self) {
        let mut services = self.services.write().await;
        services.retain(|_, e| !e.is_expired());
    }

    // --- Health status cache operations ---

    /// Insert or update a health status.
    pub async fn put_health(&self, status: HealthStatus) {
        let entry = CacheEntry {
            value: status,
            expires_at: Instant::now() + self.default_ttl,
        };
        let name = entry.value.service_name.clone();
        let mut health = self.health.write().await;
        health.insert(name, entry);
    }

    /// Get a cached health status by service name.
    pub async fn get_health(&self, name: &str) -> Option<HealthStatus> {
        let health = self.health.read().await;
        health.get(name).and_then(|e| {
            if e.is_expired() {
                None
            } else {
                Some(e.value.clone())
            }
        })
    }

    /// Clean up expired entries from health cache.
    pub async fn cleanup_health(&self) {
        let mut health = self.health.write().await;
        health.retain(|_, e| !e.is_expired());
    }

    /// Clear all cached entries.
    pub async fn clear(&self) {
        let mut services = self.services.write().await;
        let mut health = self.health.write().await;
        services.clear();
        health.clear();
    }

    /// Get cache statistics.
    pub async fn stats(&self) -> CacheStats {
        let services = self.services.read().await;
        let health = self.health.read().await;
        CacheStats {
            services_total: services.len(),
            services_expired: services.values().filter(|e| e.is_expired()).count(),
            health_total: health.len(),
            health_expired: health.values().filter(|e| e.is_expired()).count(),
        }
    }
}

impl Default for ServiceCache {
    fn default() -> Self {
        Self::new(Duration::from_secs(30))
    }
}

impl Clone for ServiceCache {
    fn clone(&self) -> Self {
        Self {
            // Caches start empty on clone (independent state)
            services: RwLock::new(HashMap::new()),
            health: RwLock::new(HashMap::new()),
            default_ttl: self.default_ttl,
        }
    }
}

/// Cache statistics snapshot.
#[derive(Debug, Clone)]
pub struct CacheStats {
    /// Total service entries (including expired).
    pub services_total: usize,
    /// Number of expired service entries.
    pub services_expired: usize,
    /// Total health entries (including expired).
    pub health_total: usize,
    /// Number of expired health entries.
    pub health_expired: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::health::ServiceState;

    #[tokio::test]
    async fn test_cache_put_get() {
        let cache = ServiceCache::new(Duration::from_secs(10));
        let info = DiscoveryInfo::new("test-service");
        cache.put_service(info.clone()).await;
        let got = cache.get_service("test-service").await;
        assert!(got.is_some());
        assert_eq!(got.unwrap().service_name, "test-service");
    }

    #[tokio::test]
    async fn test_cache_expiration() {
        let cache = ServiceCache::new(Duration::from_millis(50));
        let info = DiscoveryInfo::new("expire-test");
        cache.put_service(info).await;
        assert!(cache.get_service("expire-test").await.is_some());
        tokio::time::sleep(tokio::time::Duration::from_millis(60)).await;
        assert!(cache.get_service("expire-test").await.is_none());
    }

    #[tokio::test]
    async fn test_cache_cleanup() {
        let cache = ServiceCache::new(Duration::from_millis(50));
        cache.put_service(DiscoveryInfo::new("s1")).await;
        cache.put_service(DiscoveryInfo::new("s2")).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(60)).await;
        cache.cleanup_services().await;
        let all = cache.get_all_services().await;
        assert!(all.is_empty());
    }

    #[tokio::test]
    async fn test_cache_remove() {
        let cache = ServiceCache::new(Duration::from_secs(10));
        cache.put_service(DiscoveryInfo::new("remove-me")).await;
        assert!(cache.get_service("remove-me").await.is_some());
        cache.remove_service("remove-me").await;
        assert!(cache.get_service("remove-me").await.is_none());
    }

    #[tokio::test]
    async fn test_cache_stats() {
        let cache = ServiceCache::new(Duration::from_secs(10));
        cache.put_service(DiscoveryInfo::new("s1")).await;
        let stats = cache.stats().await;
        assert_eq!(stats.services_total, 1);
        assert_eq!(stats.services_expired, 0);
    }

    #[tokio::test]
    async fn test_cache_health() {
        let cache = ServiceCache::new(Duration::from_secs(10));
        let status = HealthStatus {
            service_name: "h1".to_string(),
            state: ServiceState::Online,
            version: "1.0".to_string(),
            timestamp: 123456,
        };
        cache.put_health(status.clone()).await;
        let got = cache.get_health("h1").await;
        assert!(got.is_some());
        assert_eq!(got.unwrap().state, ServiceState::Online);
    }

    #[tokio::test]
    async fn test_cache_clone_empty() {
        let cache = ServiceCache::new(Duration::from_secs(10));
        cache.put_service(DiscoveryInfo::new("original")).await;
        let cloned = cache.clone();
        // Cloned cache starts empty (independent state)
        assert!(cloned.get_service("original").await.is_none());
    }

    #[tokio::test]
    async fn test_cache_clear() {
        let cache = ServiceCache::new(Duration::from_secs(10));
        cache.put_service(DiscoveryInfo::new("s1")).await;
        cache.put_service(DiscoveryInfo::new("s2")).await;
        cache.clear().await;
        let all = cache.get_all_services().await;
        assert!(all.is_empty());
    }
}

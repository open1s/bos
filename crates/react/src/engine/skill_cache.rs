use super::*;

#[derive(Clone)]
/// A cached skill with its resolved instructions.
pub struct CachedSkill {
    /// Resolved skill instructions.
    pub instructions: String,
    /// Directory the skill was loaded from.
    pub skill_dir: String,
    /// When the skill was cached.
    pub loaded_at: Instant,
}

/// A TTL cache of skill instructions.
pub struct SkillCache {
    cache: Arc<DashMap<String, Arc<CachedSkill>>>,
    ttl: Duration,
}

impl SkillCache {
    /// Create a cache whose entries live for `ttl`.
    pub fn new(ttl: Duration) -> Self {
        Self {
            cache: Arc::new(DashMap::new()),
            ttl,
        }
    }

    /// Return a fresh cached skill, inserting the given body if absent or expired.
    pub fn get_or_insert(
        &self,
        skill_name: &str,
        instructions: String,
        skill_dir: String,
    ) -> Arc<CachedSkill> {
        if let Some(entry) = self.cache.get(skill_name) {
            if entry.loaded_at.elapsed() < self.ttl {
                // Cloning the Arc keeps a multi-kilobyte instruction body from
                // being copied on every skill lookup.
                return Arc::clone(entry.value());
            }
        }
        let skill = Arc::new(CachedSkill {
            instructions,
            skill_dir,
            loaded_at: Instant::now(),
        });
        self.cache
            .insert(skill_name.to_string(), Arc::clone(&skill));
        skill
    }

    /// Return a cached skill if present and unexpired.
    pub fn get(&self, skill_name: &str) -> Option<Arc<CachedSkill>> {
        // Drop the read guard before removing: removing a key needs the
        // shard's write lock, and dashmap deadlocks when both are held by the
        // same call.
        let expired = match self.cache.get(skill_name) {
            Some(entry) => {
                if entry.loaded_at.elapsed() < self.ttl {
                    return Some(Arc::clone(entry.value()));
                }
                true
            }
            None => false,
        };
        if expired {
            // Remove only if the entry is still the expired one; a concurrent
            // caller may have inserted a fresh entry while the guard was held.
            self.cache
                .remove_if(skill_name, |_, entry| entry.loaded_at.elapsed() >= self.ttl);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_expired_skill_is_not_served_and_can_be_replaced() {
        let cache = SkillCache::new(Duration::from_millis(50));
        let first = cache.get_or_insert("s", "old".to_string(), "d".to_string());
        assert_eq!(first.instructions, "old");
        assert!(cache.get("s").is_some(), "a fresh entry is served");

        std::thread::sleep(Duration::from_millis(100));

        assert!(
            cache.get("s").is_none(),
            "an expired entry is a miss, not a stale hit"
        );
        let second = cache.get_or_insert("s", "new".to_string(), "d".to_string());
        assert_eq!(second.instructions, "new");
        assert_eq!(
            cache.get("s").map(|entry| entry.instructions.clone()),
            Some("new".to_string())
        );
    }
}

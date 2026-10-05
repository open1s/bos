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
    cache: Arc<DashMap<String, CachedSkill>>,
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
                return Arc::new(entry.clone());
            } else {
                self.cache.remove(skill_name);
            }
        }
        let skill = CachedSkill {
            instructions,
            skill_dir,
            loaded_at: Instant::now(),
        };
        self.cache.insert(skill_name.to_string(), skill.clone());
        Arc::new(skill)
    }

    /// Return a cached skill if present and unexpired.
    pub fn get(&self, skill_name: &str) -> Option<Arc<CachedSkill>> {
        self.cache.get(skill_name).and_then(|entry| {
            if entry.loaded_at.elapsed() < self.ttl {
                Some(Arc::new(entry.clone()))
            } else {
                self.cache.remove(skill_name);
                None
            }
        })
    }
}

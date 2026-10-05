use super::*;

#[derive(Clone)]
pub struct CachedSkill {
    pub instructions: String,
    pub skill_dir: String,
    pub loaded_at: Instant,
}

pub struct SkillCache {
    cache: Arc<DashMap<String, CachedSkill>>,
    ttl: Duration,
}

impl SkillCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            cache: Arc::new(DashMap::new()),
            ttl,
        }
    }

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

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryRecord {
    pub thought: String,
    pub action: String,
    pub observation: Value,
}

#[cfg(test)]
mod tests_memory {
    use super::*;
    use std::fs;
    #[test]
    fn memory_serialize_roundtrip() {
        let mut mem = Memory::new();
        mem.push(
            "think".to_string(),
            "act".to_string(),
            serde_json::json!("obs1"),
        );
        mem.push(
            "think2".to_string(),
            "act2".to_string(),
            serde_json::json!("obs2"),
        );
        let path = std::env::temp_dir().join("bos_react_memory.json");
        mem.save_to_file(path.to_str().unwrap()).unwrap();
        let loaded = Memory::load_from_file(path.to_str().unwrap()).unwrap();
        assert_eq!(loaded, mem);
        let _ = fs::remove_file(path);
    }
}

#[derive(Debug, Default, Serialize, Deserialize, Clone, PartialEq)]
pub struct Memory {
    pub history: Vec<MemoryRecord>,
}

impl Memory {
    pub fn new() -> Self {
        Self {
            history: Vec::new(),
        }
    }
    pub fn push(&mut self, thought: String, action: String, observation: Value) {
        self.history.push(MemoryRecord {
            thought,
            action,
            observation,
        });
    }
    pub fn last_observation(&self) -> Option<&Value> {
        self.history.last().map(|r| &r.observation)
    }
    pub fn save_to_file(&self, path: &str) -> Result<(), std::io::Error> {
        let data = serde_json::to_string(&self.history).unwrap_or_else(|_| "[]".to_string());
        fs::write(path, data)
    }
    pub fn load_from_file(path: &str) -> Result<Self, std::io::Error> {
        let data = fs::read_to_string(path)?;
        let records: Vec<MemoryRecord> = serde_json::from_str(&data).unwrap_or_else(|_| Vec::new());
        Ok(Memory { history: records })
    }
}

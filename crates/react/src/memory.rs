use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub thought: String,
    pub action: String,
    pub observation: Value,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
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

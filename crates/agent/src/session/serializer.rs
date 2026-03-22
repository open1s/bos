use super::{AgentState, SessionError, SessionMetadata};
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use std::io::{Read, Write};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct SessionSerializer;

impl SessionSerializer {
    pub fn serialize(state: &AgentState) -> Result<Vec<u8>, SessionError> {
        serde_json::to_vec(state).map_err(|e| SessionError::Serialization(e.to_string()))
    }

    pub fn deserialize(bytes: &[u8]) -> Result<AgentState, SessionError> {
        serde_json::from_slice(bytes).map_err(|e| SessionError::Serialization(e.to_string()))
    }

    pub fn new_state(agent_id: String) -> AgentState {
        let now = Self::now_timestamp();
        AgentState {
            agent_id: agent_id.clone(),
            message_log: Vec::new(),
            context: serde_json::Value::Null,
            metadata: SessionMetadata {
                created_at: now,
                updated_at: now,
                expires_at: None,
                message_count: 0,
                agent_version: "1.0.0".to_string(),
                labels: Vec::new(),
            },
        }
    }

    pub fn update_metadata(state: &mut AgentState) {
        state.metadata.updated_at = Self::now_timestamp();
        state.metadata.message_count = state.message_log.len();
    }

    fn now_timestamp() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    pub fn is_expired(state: &AgentState) -> bool {
        if let Some(expires_at) = state.metadata.expires_at {
            let now = Self::now_timestamp();
            now >= expires_at
        } else {
            false
        }
    }

    pub fn compress(data: &[u8]) -> Result<Vec<u8>, SessionError> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(data)
            .map_err(|e| SessionError::Serialization(e.to_string()))?;
        encoder
            .finish()
            .map_err(|e| SessionError::Serialization(e.to_string()))
    }

    pub fn decompress(data: &[u8]) -> Result<Vec<u8>, SessionError> {
        let mut decoder = GzDecoder::new(data);
        let mut decompressed = Vec::new();
        decoder
            .read_to_end(&mut decompressed)
            .map_err(|e| SessionError::Serialization(e.to_string()))?;
        Ok(decompressed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_deserialize() {
        let state = SessionSerializer::new_state("test-agent".to_string());
        let bytes = SessionSerializer::serialize(&state).unwrap();
        let restored = SessionSerializer::deserialize(&bytes).unwrap();

        assert_eq!(restored.agent_id, state.agent_id);
        assert_eq!(restored.message_log.len(), state.message_log.len());
    }

    #[test]
    fn test_compression() {
        let data: Vec<u8> = (0..1000)
            .flat_map(|i| i.to_string().as_bytes().to_vec())
            .collect();
        let compressed = SessionSerializer::compress(&data).unwrap();
        let decompressed = SessionSerializer::decompress(&compressed).unwrap();

        assert_eq!(decompressed, data);
        assert!(compressed.len() < data.len());
    }

    #[test]
    fn test_is_expired() {
        let mut state = SessionSerializer::new_state("test-agent".to_string());
        assert!(!SessionSerializer::is_expired(&state));

        state.metadata.expires_at = Some(SessionSerializer::now_timestamp() - 100);
        assert!(SessionSerializer::is_expired(&state));
    }
}

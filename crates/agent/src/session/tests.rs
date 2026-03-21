//! Integration tests for Session serialization and deserialization

use super::{
    AgentState, SessionSerializer, SessionManager,
    SessionConfig, SessionMetadata, SessionSummary,
    SessionError,
};
use crate::session::storage::SessionStorage;
use tempfile::TempDir;
use std::time::{SystemTime, UNIX_EPOCH};

fn create_cache_config(base_dir: &std::path::Path) -> SessionConfig {
    SessionConfig {
        base_dir: base_dir.to_path_buf(),
        default_ttl_secs: None,
        compression_enabled: false,
    }
}


#[tokio::test]
async fn test_session_serialization_roundtrip() {
    let state = AgentState {
        agent_id: "test-agent".to_string(),
        message_log: vec![],
        context: serde_json::json!({"test": "data"}),
        metadata: SessionMetadata {
            created_at: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
            updated_at: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
            expires_at: Some(SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() + 3600),
            message_count: 0,
            agent_version: "1.0.0".to_string(),
            labels: vec!["test".to_string()],
        },
    };

    // Serialize
    let serialized = SessionSerializer::serialize(&state).unwrap();
    assert!(!serialized.is_empty());

    // Deserialize
    let deserialized = SessionSerializer::deserialize(&serialized).unwrap();
    assert_eq!(deserialized.agent_id, state.agent_id);
    assert_eq!(deserialized.metadata.agent_version, state.metadata.agent_version);
    assert_eq!(deserialized.message_log.len(), state.message_log.len());
}

#[tokio::test]
async fn test_session_storage_save_and_load() {
    let temp = TempDir::new().unwrap();
    let config = create_cache_config(temp.path());
    let storage = SessionStorage::new(config.clone());

    let state = AgentState {
        agent_id: "test-storage".to_string(),
        message_log: vec![],
        context: serde_json::json!({}),
        metadata: SessionMetadata {
            created_at: 1234567890,
            updated_at: 1234567890,
            expires_at: None,
            message_count: 0,
            agent_version: "1.0.0".to_string(),
            labels: vec![],
        },
    };

    // Save
    storage.save(&state).await.unwrap();

    // Load
    let loaded = storage.load("test-storage").await.unwrap();
    assert_eq!(loaded.agent_id, state.agent_id);
    assert_eq!(loaded.metadata.created_at, state.metadata.created_at);
}

#[tokio::test]
async fn test_session_storage_list_files() {
    let temp = TempDir::new().unwrap();
    let config = create_cache_config(temp.path());
    let storage = SessionStorage::new(config.clone());

    // Create multiple sessions
    for i in 1..=3 {
        let state = AgentState {
            agent_id: format!("session-{}", i),
            message_log: vec![],
            context: serde_json::json!({}),
        metadata: SessionMetadata {
            created_at: 1234567890,
            updated_at: 1234567890,
            expires_at: None,
            message_count: 0,
            agent_version: "1.0.0".to_string(),
            labels: vec![],
        },
        };
        storage.save(&state).await.unwrap();
    }

    // List files
    let files = storage.list_files().await.unwrap();
    assert_eq!(files.len(), 3);
}

#[tokio::test]
async fn test_session_storage_delete() {
    let temp = TempDir::new().unwrap();
    let config = create_cache_config(temp.path());
    let storage = SessionStorage::new(config.clone());

    let state = AgentState {
        agent_id: "delete-test".to_string(),
        message_log: vec![],
        context: serde_json::json!({}),
        metadata: SessionMetadata {
            created_at: 1234567890,
            updated_at: 1234567890,
            expires_at: Some(1234571490),
            message_count: 0,
            agent_version: "1.0.0".to_string(),
            labels: vec![],
        },
    };

    // Save
    storage.save(&state).await.unwrap();

    // Verify exists
    let exists = storage.exists("delete-test").await;
    assert!(exists);

    // Delete
    storage.delete("delete-test").await.unwrap();

    // Verify doesn't exist
    let exists_after = storage.exists("delete-test").await;
    assert!(!exists_after);
}

#[tokio::test]
async fn test_session_manager_crud() {
    let temp = TempDir::new().unwrap();
    let config = create_cache_config(temp.path());
    let manager = SessionManager::new(config);

    // Create
    let created = manager.create("crud-agent".to_string()).await.unwrap();
    assert_eq!(created.agent_id, "crud-agent");
    assert_eq!(created.message_log.len(), 0);

    // Get
    let retrieved = manager.get("crud-agent").await.unwrap();
    assert_eq!(retrieved.agent_id, "crud-agent");

    // Update
    let mut state = retrieved;
    state.metadata.labels.push("updated".to_string());
    state.metadata.updated_at = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    manager.update("crud-agent", state).await.unwrap();

    let updated = manager.get("crud-agent").await.unwrap();
    assert!(updated.metadata.labels.iter().any(|l| l == "updated"));

    // List
    let list = manager.list().await.unwrap();
    assert!(list.len() >= 1);
    assert!(list.iter().any(|s| s.agent_id == "crud-agent"));

    // Delete
    manager.delete("crud-agent").await.unwrap();

    let get_after = manager.get("crud-agent").await;
    assert!(matches!(get_after, Err(SessionError::NotFound(_))));
}

#[tokio::test]
async fn test_session_manager_duplicate_id() {
    let temp = TempDir::new().unwrap();
    let config = create_cache_config(temp.path());
    let manager = SessionManager::new(config);

    // Create first session
    manager.create("duplicate-agent".to_string()).await.unwrap();

    // Try to create duplicate - should fail
    let result = manager.create("duplicate-agent".to_string()).await;
    assert!(matches!(result, Err(SessionError::AlreadyExists(_))));
}

#[tokio::test]
async fn test_session_manager_persistence() {
    let temp = TempDir::new().unwrap();
    let base_dir = temp.path().to_path_buf();
    
    // Create initial session
    let config1 = SessionConfig {
        base_dir: base_dir.clone(),
        default_ttl_secs: Some(3600),
        compression_enabled: false,
    };
    
    let manager1 = SessionManager::new(config1.clone());
    let state = manager1.create("persist-agent".to_string()).await.unwrap();
    
    // Create new manager with same config (simulates restart)
    let config2 = SessionConfig {
        base_dir: base_dir.clone(),
        default_ttl_secs: Some(3600),
        compression_enabled: false,
    };
    
    let manager2 = SessionManager::new(config2);
    let retrieved = manager2.get("persist-agent").await.unwrap();
    
    assert_eq!(retrieved.agent_id, state.agent_id);
    assert_eq!(retrieved.metadata.created_at, state.metadata.created_at);
}

#[tokio::test]
async fn test_session_expired_detection() {
    let temp = TempDir::new().unwrap();
    let config = create_cache_config(temp.path());
    let storage = SessionStorage::new(config.clone());

    let state = AgentState {
        agent_id: "expired-test".to_string(),
        message_log: vec![],
        context: serde_json::json!({}),
        metadata: SessionMetadata {
            created_at: 1234567890,
            updated_at: 1234567890,
            expires_at: Some(1234567890 - 1), // Expired 1 second ago
            message_count: 0,
            agent_version: "1.0.0".to_string(),
            labels: vec![],
        },
    };

    // Save
    storage.save(&state).await.unwrap();

    // Check expiration
    let expired = SessionSerializer::is_expired(&state);
    assert!(expired);
}

#[tokio::test]
async fn test_session_metadata_update() {
    let temp = TempDir::new().unwrap();
    let config = create_cache_config(temp.path());
    let manager = SessionManager::new(config);

    manager.create("metadata-test".to_string()).await.unwrap();

    let mut state = manager.get("metadata-test").await.unwrap();
    let original_updated = state.metadata.updated_at;
    
    // Update metadata
    SessionSerializer::update_metadata(&mut state);
    
    assert!(state.metadata.updated_at >= original_updated);
    
    // Save updated state
    manager.update("metadata-test", state.clone()).await.unwrap();

    // Verify persisted
    let retrieved = manager.get("metadata-test").await.unwrap();
    assert_eq!(retrieved.agent_id, state.agent_id);
}

#[tokio::test]
async fn test_session_with_messages() {
    let temp = TempDir::new().unwrap();
    let config = create_cache_config(temp.path());
    let manager = SessionManager::new(config);

    let mut state = SessionSerializer::new_state("message-test".to_string());

    // Add messages
    state.message_log.push(crate::agent::Message::User("Hello".to_string()));
    state.message_log.push(crate::agent::Message::Assistant("Hi there!".to_string()));
    
    SessionSerializer::update_metadata(&mut state);

    manager.storage.save(&state).await.unwrap();

    let retrieved = manager.storage.load("message-test").await.unwrap();
    assert_eq!(retrieved.message_log.len(), 2);
    match &retrieved.message_log[0] {
        crate::agent::Message::User(content) => assert_eq!(content, "Hello"),
        _ => panic!("Expected User message"),
    }
    match &retrieved.message_log[1] {
        crate::agent::Message::Assistant(content) => assert_eq!(content, "Hi there!"),
        _ => panic!("Expected Assistant message"),
    }
}

#[test]
fn test_session_config_defaults() {
    let config = SessionConfig::default();
    
    assert_eq!(config.base_dir, std::path::PathBuf::from(".bos/sessions"));
    assert!(config.default_ttl_secs.is_none());
    assert!(!config.compression_enabled);
}

#[tokio::test]
async fn test_session_summary_metadata() {
    let temp = TempDir::new().unwrap();
    let config = create_cache_config(temp.path());
    let manager = SessionManager::new(config);

    manager.create("summary-test".to_string()).await.unwrap();
    manager.update("summary-test", {
        let mut s = manager.get("summary-test").await.unwrap();
        s.metadata.labels.push("label1".to_string());
        s.metadata.labels.push("label2".to_string());
        s
    }).await.unwrap();

    let list = manager.list().await.unwrap();
    let summary = list.iter().find(|s| s.agent_id == "summary-test").unwrap();
    
    assert_eq!(summary.labels.len(), 2);
    assert!(summary.labels.contains(&"label1".to_string()));
    assert!(summary.labels.contains(&"label2".to_string()));
}

#[tokio::test]
async fn test_session_cache_performance() {
    let temp = TempDir::new().unwrap();
    let config = create_cache_config(temp.path());
    let manager = SessionManager::new(config);

    // Create session
    manager.create("cache-test".to_string()).await.unwrap();

    // First read - loads from storage
    let start = std::time::Instant::now();
    let _first = manager.get("cache-test").await.unwrap();
    let first_duration = start.elapsed();

    // Second read - loads from cache
    let start = std::time::Instant::now();
    let _second = manager.get("cache-test").await.unwrap();
    let second_duration = start.elapsed();

    // Cache should be faster or equal (may not be significant with simple operations)
    assert!(second_duration <= first_duration);
}

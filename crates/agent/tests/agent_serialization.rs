use agent::AgentConfig;
use qserde::prelude::*;

#[test]
fn test_agent_config_serialization() {
    let config = AgentConfig::default();
    let bytes = config.dump().expect("AgentConfig should be serializable");
    let loaded = bytes
        .load::<AgentConfig>()
        .expect("AgentConfig should be deserializable");
    assert_eq!(loaded.name, config.name);
    assert_eq!(loaded.model, config.model);
}

#[test]
fn test_agent_config_has_archive_derive() {
    let config = AgentConfig::default();
    let bytes = config.dump().expect("AgentConfig should serialize");
    assert!(!bytes.is_empty(), "Serialized bytes should not be empty");
}

#[test]
fn test_agent_has_archive_derive() {
    let config = AgentConfig::default();
    let bytes = config.dump().expect("AgentConfig should serialize");
    assert!(!bytes.is_empty(), "Serialized bytes should not be empty");
}

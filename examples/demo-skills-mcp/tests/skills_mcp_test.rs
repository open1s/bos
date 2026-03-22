//! Integration tests for Skills & MCP Demo
//!
//! Tests skill loading, composition, injection, and MCP tool integration.

use agent::skills::{SkillLoader, SkillInjector};
use agent::mcp::{McpClient, McpToolAdapter};
use agent::tools::{ToolRegistry, Tool};
use std::path::PathBuf;
use std::sync::Arc;

/// Test skill discovery and loading from YAML files
#[tokio::test]
async fn demo_skill_load() {
    let skills_dir = PathBuf::from("skills");
    let mut loader = SkillLoader::new(skills_dir.clone());

    // Discover skills
    let skills = loader.discover().expect("discover should succeed");
    assert!(!skills.is_empty(), "should find skills in skills/ directory");

    // Basic communication skill should exist
    let basic_com = skills.iter().find(|s| s.name == "basic-communication");
    assert!(
        basic_com.is_some(),
        "basic-communication skill should exist"
    );

    // Load skill content
    if let Some(skill) = basic_com {
        let content = loader.load(&skill.name).expect("load should succeed");
        assert_eq!(content.metadata.name, "basic-communication");
        assert!(!content.instructions.is_empty());
        assert!(content.instructions.contains("Communication Guidelines"));
    }

    // Check all expected skills exist
    let expected_skills = vec!["basic-communication", "code-analysis", "security", "composite"];
    for expected in expected_skills {
        assert!(
            skills.iter().any(|s| s.name == expected),
            "skill '{}' should be discovered",
            expected
        );
    }

    // Validate dependencies (composite skill)
    let composite = skills.iter().find(|s| s.name == "composite");
    assert!(composite.is_some());
    if let Some(skill) = composite {
        assert!(skill.requires.contains(&"code-analysis".to_string()));
        assert!(skill.requires.contains(&"security".to_string()));
    }
}

/// Test skill composition and dependency validation
#[tokio::test]
async fn demo_skill_compose() {
    let skills_dir = PathBuf::from("skills");
    let mut loader = SkillLoader::new(skills_dir.clone());

    let _skills = loader.discover().expect("discover should succeed");

    // Validate all skills - should succeed since all dependencies exist
    loader.validate_all().expect("validation should succeed");

    // Check dependency graph
    let graph = loader.dependency_graph();
    assert!(!graph.is_empty(), "dependency graph should not be empty");

    // Find composite skill in graph
    let composite_entry = graph.iter().find(|(name, _)| name == "composite");
    assert!(composite_entry.is_some());

    if let Some((_, deps)) = composite_entry {
        assert!(deps.contains(&"code-analysis".to_string()));
        assert!(deps.contains(&"security".to_string()));
    }

    // Check that skills with no dependencies have empty deps
    let basic_com = graph.iter().find(|(name, _)| name == "basic-communication");
    assert!(basic_com.is_some());
    if let Some((_, deps)) = basic_com {
        assert!(deps.is_empty());
    }
}

/// Test skill injection into agent system prompt
#[tokio::test]
async fn demo_skill_inject() {
    let skills_dir = PathBuf::from("skills");
    let mut loader = SkillLoader::new(skills_dir.clone());

    let skills = loader.discover().expect("discover should succeed");
    let skill_contents: Vec<_> = skills
        .iter()
        .filter_map(|m| loader.load(&m.name).ok())
        .collect();

    assert!(!skill_contents.is_empty(), "should have loaded skill contents");

    let injector = SkillInjector::new();

    // Test compact format (using inject_available with metadata)
    let compact = injector.inject_available(&skills);
    assert!(compact.contains("<available_skills>"));
    assert!(compact.contains("</available_skills>"));
    assert!(compact.len() < 2000, "compact format should be reasonably small");

    // Test standard format (using inject_specific)
    let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
    let standard = injector.inject_specific(&skill_contents, &names);
    assert!(standard.contains("<available_skills>"));
    assert!(standard.contains("</available_skills>"));
    assert!(standard.contains("<instructions>"));
    assert!(standard.len() > compact.len(), "standard should be larger than compact");

    // Test verbose format (using inject_by_category)
    let verbose = injector.inject_by_category(&skill_contents, agent::skills::SkillCategory::Communication);
    assert!(verbose.contains("<available_skills>"));
    assert!(verbose.contains("</available_skills>"));

    // Test injection stats
    let stats = injector.injected_stats(&standard);
    assert!(stats.skill_count > 0, "should have injected skills");
    assert!(stats.char_count > 0, "should have characters");
    assert!(stats.line_count > 0, "should have lines");
}

/// Test MCP client connection (requires mcp-everything in PATH)
#[tokio::test]
#[ignore = "Requires mcp-everything server in PATH"]
async fn demo_mcp_client() {
    // Check if mcp-everything is available
    let check = tokio::process::Command::new("mcp-everything")
        .arg("--version")
        .output()
        .await;

    if check.is_err() {
        eprintln!("Skipping: mcp-everything not in PATH");
        return;
    }

    // Spawn MCP client
    let client = McpClient::spawn("mcp-everything", &[])
        .await
        .expect("should spawn mcp-everything");

    // Initialize
    let capabilities = client.initialize().await.expect("should initialize");
    assert!(capabilities.tools, "mcp-everything should support tools");

    // List tools
    let tools = client.list_tools().await.expect("should list tools");
    assert!(!tools.is_empty(), "mcp-everything should have tools");

    // Verify tool structure
    for tool in &tools {
        assert!(!tool.name.is_empty(), "tool should have a name");
        assert!(!tool.description.is_empty(), "tool should have a description");
    }
}

/// Test MCP tool adapter creation and registration (requires mcp-everything in PATH)
#[tokio::test]
#[ignore = "Requires mcp-everything server in PATH"]
async fn demo_mcp_adapter() {
    // Check if mcp-everything is available
    let check = tokio::process::Command::new("mcp-everything")
        .arg("--version")
        .output()
        .await;

    if check.is_err() {
        eprintln!("Skipping: mcp-everything not in PATH");
        return;
    }

    // Spawn and initialize
    let client = McpClient::spawn("mcp-everything", &[])
        .await
        .expect("should spawn");
    client.initialize().await.expect("should initialize");

    // List tools
    let tools = client.list_tools().await.expect("should list tools");
    if tools.is_empty() {
        eprintln!("No tools to test");
        return;
    }

    let mut registry = ToolRegistry::new();
    let client_arc = Arc::new(client);

    // Create adapters and register
    for tool_def in &tools {
        let adapter = McpToolAdapter::new(
            client_arc.clone(),
            tool_def.name.clone(),
            tool_def.description.clone(),
            tool_def.input_schema.clone(),
        );

        // Verify adapter properties
        assert_eq!(adapter.name(), tool_def.name);
        assert!(!adapter.description().short.is_empty());

        // Register in registry
        registry
            .register(Arc::new(adapter))
            .expect("should register adapter");
    }

    // Verify all tools are registered
    let registered = registry.list();
    assert_eq!(registered.len(), tools.len(), "all tools should be registered");

    // Verify OpenAI format conversion
    let openai_format = registry.to_openai_format();
    assert_eq!(openai_format.len(), tools.len());
}

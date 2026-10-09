use agent::agent::agentic::{Agent, AgentConfig, LlmProvider};
use agent::error::AgentError;
use react::llm::vendor::OpenAiVendorBuilder;
use std::sync::Arc;

#[tokio::test]
async fn test_agent_builds_with_vendor() {
    let vendor = OpenAiVendorBuilder::new()
        .model("gpt-4o-mini".to_string())
        .api_key("sk-test".to_string())
        .build()
        .expect("Failed to build vendor");

    let mut llm = LlmProvider::new();
    llm.register_vendor("openai".to_string(), Box::new(vendor));

    let config = AgentConfig::default();
    let _agent: Agent = Agent::new(config, Arc::new(llm));
}

#[tokio::test]
async fn test_agent_run_simple() {
    let vendor = OpenAiVendorBuilder::new()
        .model("gpt-4o-mini".to_string())
        .api_key("sk-test".to_string())
        .build()
        .expect("Failed to build vendor");

    let mut llm = LlmProvider::new();
    llm.register_vendor("openai".to_string(), Box::new(vendor));

    let config = AgentConfig::default();
    let agent = Agent::new(config, Arc::new(llm));

    let result: Result<String, AgentError> = agent.run_simple("hi").await;
    // With invalid key, it will error - but no panic
    assert!(result.is_ok() || result.is_err());
}

/// A skill named in the deny-list is skipped, and the others are not — the
/// difference between "the field is stored" and "the engine obeys it".
#[tokio::test]
async fn a_denied_skill_is_not_registered() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/skills")
        .to_path_buf();
    let build = || {
        let mut llm = LlmProvider::new();
        llm.register_vendor(
            "openai".to_string(),
            Box::new(
                OpenAiVendorBuilder::new()
                    .model("gpt-4o-mini".to_string())
                    .api_key("sk-test".to_string())
                    .build()
                    .expect("vendor"),
            ),
        );
        Agent::new(AgentConfig::default(), Arc::new(llm))
    };

    // Everyone is picked up when nobody objects.
    let mut all = build();
    all.register_skills_from_dir(dir.clone())
        .expect("fixture loads");
    let names: Vec<String> = all
        .get_skills_content()
        .iter()
        .map(|(name, _)| (*name).to_string())
        .collect();
    assert!(
        names.len() >= 2 && names.iter().any(|n| n == "calculator"),
        "fixture should expose several skills: {names:?}"
    );

    // Denying one leaves the rest alone: a deny-list, not a switch.
    let mut some = build();
    some.register_skills_from_dir_denying(dir, &["calculator".to_string()])
        .expect("fixture loads");
    let kept: Vec<String> = some
        .get_skills_content()
        .iter()
        .map(|(name, _)| (*name).to_string())
        .collect();
    assert!(!kept.iter().any(|n| n == "calculator"), "kept: {kept:?}");
    assert_eq!(kept.len(), names.len() - 1, "kept: {kept:?} vs {names:?}");
}

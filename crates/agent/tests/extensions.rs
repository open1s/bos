//! Integration coverage for the common extension substrate and both shims.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent::agent::plugin::{AgentPlugin, StreamTokenWrapper};
use agent::extensions::{
    Capability, Contribution, Extension, ExtensionError, ExtensionRegistry, ExtensionStandard,
    HookContribution, HookStage, RefusedFeature,
};
use agent::plugins::{ItemKind, PluginRegistry};
use agent::{Agent, AgentConfig};
use async_trait::async_trait;
use serde_json::json;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/extensions")
}

fn fixture(name: &str) -> PathBuf {
    fixtures().join(name)
}

fn installed(name: &str) -> ExtensionRegistry {
    let mut registry = ExtensionRegistry::new();
    registry
        .install_package(&fixture(name))
        .unwrap_or_else(|error| panic!("{name} installs: {error}"));
    registry
}

#[test]
fn a_dsh_package_is_discovered_and_parsed() {
    let package = ExtensionRegistry::load(&fixture("dsh-echo")).expect("loads");
    let extension = &package.extension;
    assert_eq!(extension.id, "@local/dsh-echo");
    assert_eq!(extension.name, "Fixture DSH Echo");
    assert_eq!(extension.version, "1.2.3");
    assert_eq!(extension.standard, ExtensionStandard::DeepSeekHarness);
    assert!(extension.has_capability(Capability::Tools));
    assert!(extension.has_capability(Capability::Commands));
    assert!(extension.has_capability(Capability::Skills));
    assert!(extension.has_capability(Capability::Prompts));
    assert!(extension.has_capability(Capability::Panels));
    assert!(extension.has_capability(Capability::Hooks));

    assert_eq!(extension.details.manifest_version, Some(1));
    assert_eq!(extension.details.bundle_rows, vec!["dsh-echo".to_string()]);
    assert_eq!(
        extension
            .details
            .client
            .as_ref()
            .map(|client| client.platform.as_str()),
        Some("web")
    );
    assert!(extension.details.host_entry.is_some());
    assert!(extension.details.client_entry.is_some());
    assert!(extension.details.icon.is_some());
    assert_eq!(
        extension.details.bundle_patch.as_ref(),
        Some(&fixture("dsh-echo").join("cordis.patch.yml"))
    );
    assert!(package
        .refusals
        .iter()
        .any(|refusal| refusal.feature == RefusedFeature::BundleMount));

    let mut registry = ExtensionRegistry::new();
    registry.install(package).expect("installs");
    assert_eq!(
        registry.tool_names(),
        vec!["dsh_echo".to_string(), "dsh_echo__hello".to_string()]
    );
    assert_eq!(registry.commands().len(), 1);
    assert_eq!(registry.commands()[0].name, "hello");
    assert_eq!(registry.skills().len(), 1);
    assert_eq!(registry.skills()[0].name, "greet");
    assert_eq!(registry.panels().len(), 1);
    assert_eq!(registry.panels()[0].id, "@local/dsh-echo");
    assert_eq!(registry.prompts().len(), 1);
    assert_eq!(registry.prompts()[0].name, "hello");
    assert_eq!(
        registry.prompts()[0].arguments_hint.as_deref(),
        Some("$ARGUMENTS")
    );
}

#[test]
fn a_codex_package_is_discovered_and_parsed() {
    let package = ExtensionRegistry::load(&fixture("codex-deploy")).expect("loads");
    let extension = &package.extension;
    assert_eq!(extension.id, "codex-deploy");
    assert_eq!(extension.name, "Codex Deploy Fixture");
    assert_eq!(extension.version, "0.4.0");
    assert_eq!(extension.standard, ExtensionStandard::Codex);
    assert!(extension.has_capability(Capability::Tools));
    assert!(extension.has_capability(Capability::Commands));
    assert!(extension.has_capability(Capability::Skills));
    assert!(extension.has_capability(Capability::Prompts));
    assert!(extension.details.icon.is_some());

    let mut registry = ExtensionRegistry::new();
    registry.install(package).expect("installs");
    assert_eq!(
        registry.tool_names(),
        vec!["codex_deploy__deploy".to_string()]
    );
    assert_eq!(registry.commands()[0].name, "deploy");
    assert_eq!(registry.skills()[0].name, "codex-greet");
    let prompts: Vec<&str> = registry
        .prompts()
        .iter()
        .map(|prompt| prompt.name.as_str())
        .collect();
    assert!(prompts.contains(&"deploy"));
    assert!(prompts.contains(&"reviewer"));
    let reviewer = registry
        .prompts()
        .into_iter()
        .find(|prompt| prompt.name == "reviewer")
        .expect("the agent prompt");
    assert!(reviewer.body.contains("review changes"));
}

#[test]
fn a_tool_from_each_standard_is_callable_through_the_agent_registry() {
    let mut registry = ExtensionRegistry::new();
    registry.install_package(&fixture("dsh-echo")).expect("dsh");
    registry
        .install_package(&fixture("codex-deploy"))
        .expect("codex");
    let mut agent = Agent::from_config(AgentConfig::default());
    registry.apply_to_agent(&mut agent).expect("applies");

    let dsh_tool = agent
        .registry()
        .expect("tool registry")
        .get("dsh_echo")
        .expect("the DSH tool");
    let output = dsh_tool.run(&json!({ "value": "hi" })).expect("runs");
    assert_eq!(output["value"], "hi");

    let codex_tool = agent
        .registry()
        .expect("tool registry")
        .get("codex_deploy__deploy")
        .expect("the Codex prompt tool");
    let output = codex_tool
        .run(&json!({ "arguments": "production" }))
        .expect("runs");
    let prompt = output["prompt"].as_str().expect("a rendered prompt");
    assert!(
        prompt.contains("Deploy the project with these arguments: production"),
        "{prompt}"
    );
}

#[test]
fn the_declarative_tool_schema_is_preserved() {
    let registry = installed("dsh-echo");
    let tool = registry.tool("dsh_echo").expect("tool");
    let schema = tool.json_schema();
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["value"]["type"], "string");
    assert_eq!(schema["required"][0], "value");
}

#[test]
fn an_imperative_dsh_entry_is_refused_with_a_named_error() {
    let package = ExtensionRegistry::load(&fixture("dsh-echo")).expect("loads");
    assert!(!package.is_fully_supported());
    let host_refusal = package
        .refusals
        .iter()
        .find(|refusal| refusal.feature == RefusedFeature::ImperativeHostEntry)
        .expect("the host entry refusal");
    let message = host_refusal.to_string();
    assert!(message.contains("@local/dsh-echo"), "{message}");
    assert!(message.contains("index.js"), "{message}");
    assert!(message.contains("JavaScript"), "{message}");
    assert!(message.contains("ctx.tools.register"), "{message}");

    let error = package.into_strict().expect_err("strict load refuses");
    let message = error.to_string();
    assert!(message.contains("@local/dsh-echo"), "{message}");
    assert!(message.contains("index.js"), "{message}");
}

#[test]
fn a_codex_mcp_server_and_app_are_refused_without_leaking_secrets() {
    let package = ExtensionRegistry::load(&fixture("codex-deploy")).expect("loads");
    assert!(!package.is_fully_supported());
    let refusals: Vec<String> = package.refusals.iter().map(ToString::to_string).collect();
    assert!(
        refusals
            .iter()
            .any(|message| message.contains("fixture-server")),
        "{refusals:?}"
    );
    assert!(
        refusals
            .iter()
            .any(|message| message.contains("fixture-app")),
        "{refusals:?}"
    );
    for message in &refusals {
        assert!(message.contains("codex-deploy"), "{message}");
        assert!(!message.contains("sk-fixture-secret"), "{message}");
    }

    let error = package.into_strict().expect_err("strict load refuses");
    let message = error.to_string();
    assert!(message.contains("codex-deploy"), "{message}");
    assert!(message.contains("MCP server"), "{message}");
    assert!(!message.contains("sk-fixture-secret"), "{message}");
}

#[test]
fn a_dsh_package_is_not_read_as_codex() {
    let error = ExtensionRegistry::load_as(&fixture("dsh-echo"), ExtensionStandard::Codex)
        .expect_err("wrong standard");
    match error {
        ExtensionError::WrongStandard {
            expected, found, ..
        } => {
            assert_eq!(expected, ExtensionStandard::Codex);
            assert_eq!(found, ExtensionStandard::DeepSeekHarness);
        }
        other => panic!("expected WrongStandard, got {other:?}"),
    }
    let message = ExtensionRegistry::load_as(&fixture("dsh-echo"), ExtensionStandard::Codex)
        .expect_err("wrong standard")
        .to_string();
    assert!(message.contains("DeepSeek Harness"), "{message}");
    assert!(message.contains("Codex"), "{message}");
}

#[test]
fn a_codex_package_is_not_read_as_dsh() {
    let error =
        ExtensionRegistry::load_as(&fixture("codex-deploy"), ExtensionStandard::DeepSeekHarness)
            .expect_err("wrong standard");
    match error {
        ExtensionError::WrongStandard {
            expected, found, ..
        } => {
            assert_eq!(expected, ExtensionStandard::DeepSeekHarness);
            assert_eq!(found, ExtensionStandard::Codex);
        }
        other => panic!("expected WrongStandard, got {other:?}"),
    }
}

#[test]
fn a_directory_with_neither_marker_is_unknown() {
    let error = ExtensionRegistry::load(&fixture("neither")).expect_err("unknown");
    assert!(matches!(error, ExtensionError::UnknownStandard { .. }));
}

#[test]
fn discovery_reads_both_standards_and_skips_other_directories() {
    let mut registry = ExtensionRegistry::new();
    let mut installed = registry.discover(&fixtures()).expect("discovers");
    installed.sort();
    assert_eq!(
        installed,
        vec!["@local/dsh-echo".to_string(), "codex-deploy".to_string()]
    );
    assert_eq!(registry.by_standard(ExtensionStandard::Codex).len(), 1);
    assert_eq!(
        registry
            .by_standard(ExtensionStandard::DeepSeekHarness)
            .len(),
        1
    );
}

#[test]
fn the_registry_rejects_a_duplicate_id_and_leaves_itself_untouched() {
    let first = ExtensionRegistry::load(&fixture("dsh-echo")).expect("loads");
    let second = ExtensionRegistry::load(&fixture("dsh-echo")).expect("loads");
    let mut registry = ExtensionRegistry::new();
    registry.install(first).expect("first installs");
    let error = registry.install(second).expect_err("duplicate id");
    assert!(matches!(error, ExtensionError::DuplicateExtension { .. }));
    assert_eq!(registry.len(), 1);
}

#[test]
fn the_registry_rejects_a_duplicate_tool_name_across_extensions() {
    let registry = installed("dsh-echo");
    let tool = registry.tool("dsh_echo").expect("tool");
    let mut registry = registry;
    let mut second = Extension::new(
        "second",
        "Second",
        "1.0.0",
        ExtensionStandard::Codex,
        "/tmp/second",
    )
    .with_contribution(Contribution::Tool(tool));
    second = second.with_capability(Capability::Tools);
    let error = registry.add(second).expect_err("duplicate tool");
    match error {
        ExtensionError::DuplicateContribution { name, .. } => assert_eq!(name, "dsh_echo"),
        other => panic!("expected DuplicateContribution, got {other:?}"),
    }
    assert_eq!(registry.len(), 1);
}

#[test]
fn host_commands_and_panels_install_through_the_existing_plugin_registry() {
    let registry = installed("dsh-echo");
    let mut host = PluginRegistry::new();
    let installed = registry
        .install_host_items(&mut host)
        .expect("host install");
    assert_eq!(installed.commands, vec!["hello".to_string()]);
    assert_eq!(installed.panels, vec!["@local/dsh-echo".to_string()]);
    assert_eq!(
        host.owner_of("hello"),
        Some((ItemKind::Command, "@local/dsh-echo"))
    );
    assert_eq!(
        host.owner_of("@local/dsh-echo"),
        Some((ItemKind::Panel, "@local/dsh-echo"))
    );
}

struct Upper;

#[async_trait]
impl AgentPlugin for Upper {
    fn name(&self) -> &str {
        "upper"
    }

    async fn on_stream_token(&self, token: StreamTokenWrapper) -> Option<StreamTokenWrapper> {
        match token {
            StreamTokenWrapper::Text(text) => Some(StreamTokenWrapper::Text(text.to_uppercase())),
            other => Some(other),
        }
    }
}

#[tokio::test]
async fn a_hook_contribution_reaches_the_agents_plugin_registry() {
    let mut registry = ExtensionRegistry::new();
    let extension = Extension::new(
        "hook-ext",
        "Hook extension",
        "1.0.0",
        ExtensionStandard::Codex,
        "/tmp/hook-ext",
    )
    .with_capability(Capability::Hooks)
    .with_contribution(Contribution::Hook(HookContribution::new(
        Arc::new(Upper),
        vec![HookStage::StreamToken],
    )));
    registry.add(extension).expect("registers");

    let mut agent = Agent::from_config(AgentConfig::default());
    let applied = registry.apply_to_agent(&mut agent).expect("applies");
    assert_eq!(applied.hooks, vec!["upper".to_string()]);
    assert_eq!(agent.plugins().len(), 1);

    let output = agent
        .plugins()
        .on_stream_token(StreamTokenWrapper::Text("hi".to_string()))
        .await
        .expect("no veto");
    assert!(matches!(output, StreamTokenWrapper::Text(text) if text == "HI"));
}

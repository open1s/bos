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
        vec![
            "@local/dsh-echo".to_string(),
            "@local/dsh-zen".to_string(),
            "codex-deploy".to_string()
        ]
    );
    assert_eq!(registry.by_standard(ExtensionStandard::Codex).len(), 1);
    assert_eq!(
        registry
            .by_standard(ExtensionStandard::DeepSeekHarness)
            .len(),
        2,
        "the echo fixture and the model-provider fixture"
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

/// A model-provider package declares its endpoint declaratively, and the
/// imperative adapter it ALSO ships stays refused.
///
/// This is the shape the real `@opencode2dsh/dsh-plugin` has: an
/// `apply(ctx, config)` that calls `ctx.llm.registerAdapter([id], adapter)`
/// beside the declarative carrier. The capability is reported either way,
/// because the package asked for models and the host must say so.
#[test]
fn a_model_provider_package_contributes_providers_and_still_refuses_its_script() {
    let package = ExtensionRegistry::load(&fixture("dsh-provider")).expect("loads");
    let extension = &package.extension;

    assert!(
        extension.has_capability(Capability::Models),
        "the adapter call and the declarative carrier both declare models"
    );

    let providers: Vec<&agent::extensions::ProviderContribution> = extension
        .contributions
        .iter()
        .filter_map(|contribution| match contribution {
            Contribution::Provider(provider) => Some(provider),
            _ => None,
        })
        .collect();
    assert_eq!(providers.len(), 1, "one declared provider");
    let provider = providers[0];
    assert_eq!(provider.id, "zen");
    assert_eq!(provider.base_url, "https://opencode.ai/zen/v1");
    assert_eq!(provider.api_mode.as_deref(), Some("chat"));
    assert!(
        !provider.requires_key,
        "the package declared an anonymous lane"
    );
    let ids: Vec<&str> = provider
        .models
        .iter()
        .map(|model| model.id.as_str())
        .collect();
    assert_eq!(
        ids,
        vec![
            "deepseek-v4.1-flash-free",
            "glm-5.3-flash-free",
            "nemotron-3.5-super-vl-free"
        ],
        "declaration order is preserved"
    );
    let vision = provider
        .models
        .iter()
        .find(|model| model.id == "nemotron-3.5-super-vl-free")
        .expect("the vision model");
    assert_eq!(
        vision.input_types.as_deref(),
        Some(["text".to_string(), "image".to_string()].as_slice())
    );

    // The script is still refused, and the refusal says what DID land - a
    // package whose models work must not read as wholly broken.
    let refusal = package
        .refusals
        .iter()
        .find(|refusal| refusal.feature == RefusedFeature::ImperativeHostEntry)
        .expect("the host entry is refused");
    assert!(
        refusal.reason.contains("providers/*.json"),
        "the refusal names the carrier that did load: {}",
        refusal.reason
    );
    let client = package
        .refusals
        .iter()
        .find(|refusal| refusal.feature == RefusedFeature::ImperativeClientEntry)
        .expect("the client entry is refused");
    assert!(
        !client.reason.contains("providers/*.json"),
        "the client refusal does not borrow the model sentence: {}",
        client.reason
    );
}

/// A package that declares a provider with no models, no base URL, or a
/// non-HTTP base URL is refused by name rather than loading half an endpoint.
#[test]
fn a_provider_descriptor_is_validated_before_it_loads() {
    let scratch = std::env::temp_dir().join(format!("dsh-provider-bad-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);

    let write = |body: &str| {
        std::fs::create_dir_all(scratch.join("providers")).expect("the descriptor directory");
        std::fs::write(
            scratch.join("package.json"),
            json!({
                "name": "@local/bad",
                "version": "1.0.0",
                "dsh": { "manifestVersion": 1 },
            })
            .to_string(),
        )
        .expect("the manifest");
        std::fs::write(scratch.join("providers/one.json"), body).expect("the descriptor");
    };

    write(r#"{"id":"zen","base_url":"https://zen.example/v1","models":[]}"#);
    let error = ExtensionRegistry::load(&scratch).expect_err("no models is refused");
    assert!(
        error.to_string().contains("at least one entry"),
        "the message names the missing list: {error}"
    );

    write(r#"{"base_url":"https://zen.example/v1","models":[{"id":"a"}]}"#);
    let error = ExtensionRegistry::load(&scratch).expect_err("no id is refused");
    assert!(
        error.to_string().contains("id"),
        "the message names the missing id: {error}"
    );

    write(r#"{"id":"zen","base_url":"file:///etc/passwd","models":[{"id":"a"}]}"#);
    let error = ExtensionRegistry::load(&scratch).expect_err("a non-HTTP URL is refused");
    assert!(
        error.to_string().contains("http"),
        "the message names the scheme rule: {error}"
    );

    write(r#"{"id":"zen","models":[{"id":"a"}]}"#);
    let error = ExtensionRegistry::load(&scratch).expect_err("no base URL is refused");
    assert!(
        error.to_string().contains("base_url"),
        "the message names the missing URL: {error}"
    );

    let _ = std::fs::remove_dir_all(&scratch);
}

/// The registry hands a host every declared provider with its owner, and no
/// provider is installed onto the AGENT: a model endpoint is a host surface,
/// not a tool or a hook.
#[test]
fn declared_providers_are_readable_by_the_host_and_not_applied_to_the_agent() {
    let registry = installed("dsh-provider");
    let providers = registry.providers();
    assert_eq!(providers.len(), 1, "one provider is reported");
    assert_eq!(providers[0].0.id, "@local/dsh-zen", "with its owner");
    assert_eq!(providers[0].1.id, "zen");

    let mut agent = Agent::from_config(AgentConfig::default());
    let applied = registry.apply_to_agent(&mut agent).expect("applies");
    assert!(applied.tools.is_empty(), "a provider is not a tool");
    assert!(applied.hooks.is_empty(), "a provider is not a hook");
}

/// A Codex package reads the same declarative carrier. A Codex manifest has no
/// provider field at all, so a package that contributes one does it here, and
/// nothing about the file is standard-specific.
#[test]
fn the_codex_shim_reads_the_same_provider_carrier() {
    let scratch = std::env::temp_dir().join(format!("dsh-provider-codex-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(scratch.join(".codex-plugin")).expect("the manifest directory");
    std::fs::create_dir_all(scratch.join("providers")).expect("the descriptor directory");
    std::fs::write(
        scratch.join(".codex-plugin/plugin.json"),
        json!({ "name": "codex-zen", "version": "1.0.0" }).to_string(),
    )
    .expect("the manifest");
    std::fs::write(
        scratch.join("providers/zen.json"),
        json!({
            "id": "zen",
            "base_url": "https://zen.example/v1",
            "models": [{ "id": "a-free" }],
        })
        .to_string(),
    )
    .expect("the descriptor");

    let package = ExtensionRegistry::load(&scratch).expect("loads");
    assert_eq!(package.extension.standard, ExtensionStandard::Codex);
    assert!(package.extension.has_capability(Capability::Models));
    assert_eq!(
        package
            .extension
            .contributions
            .iter()
            .filter(|contribution| matches!(contribution, Contribution::Provider(_)))
            .count(),
        1
    );
    let _ = std::fs::remove_dir_all(&scratch);
}

//! Integration coverage for the common extension substrate and both shims.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent::agent::plugin::{AgentPlugin, StreamTokenWrapper};
use agent::extensions::{
    Capability, Contribution, Extension, ExtensionError, ExtensionRegistry, ExtensionStandard,
    HookContribution, HookStage, RefusedFeature, Trust,
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
            "@local/dsh-bridge".to_string(),
            "@local/dsh-echo".to_string(),
            "@local/dsh-hanging".to_string(),
            "@local/dsh-script-only".to_string(),
            "@local/dsh-throwing".to_string(),
            "@local/dsh-untrusted".to_string(),
            "@local/dsh-zen".to_string(),
            "codex-deploy".to_string()
        ]
    );
    assert_eq!(registry.by_standard(ExtensionStandard::Codex).len(), 1);
    // The bridge fixtures are read by DISCOVERY, which runs no entry: discovery
    // is a filesystem walk, and executing anything during it would mean merely
    // listing a directory runs a stranger's code.
    assert_eq!(
        registry
            .by_standard(ExtensionStandard::DeepSeekHarness)
            .len(),
        7,
        "the echo, provider and four bridge fixtures"
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

/// A package whose whole payload is an imperative host entry - the shape
/// every real DSH package ships, because the DSH manifest has no field for
/// tools or models at all - declares capabilities it cannot deliver.
///
/// The declaration and the delivery are two different facts. The host entry
/// is refused, so nothing loads, and a row that printed the declaration alone
/// would tell the reader the package can select a model it cannot.
#[test]
fn a_script_only_package_declares_more_than_it_delivers() {
    let package = ExtensionRegistry::load(&fixture("dsh-script-only")).expect("loads");
    let extension = &package.extension;

    assert!(
        extension.has_capability(Capability::Models),
        "the host entry asks for models, so the package declares the capability"
    );
    assert!(
        package
            .refusals
            .iter()
            .any(|refusal| refusal.feature == RefusedFeature::ImperativeHostEntry),
        "the host entry is refused, because this build has no JavaScript runtime"
    );

    assert!(
        extension.loaded_capabilities().is_empty(),
        "nothing loaded, so the delivered set is empty - a package that delivered \
         nothing must not claim a capability it refused to deliver"
    );
    assert!(
        extension.contributions.is_empty(),
        "the refused entry contributes nothing"
    );
}

/// The declared set and the loaded set are separate, and the loaded set is
/// derived from the contributions rather than tracked beside them.
#[test]
fn the_loaded_capabilities_follow_the_contributions() {
    let package = ExtensionRegistry::load(&fixture("dsh-provider")).expect("loads");
    let extension = &package.extension;

    assert!(
        extension.has_capability(Capability::Models),
        "the declarative carrier declares models"
    );
    // The order is the enum's own `Ord` order, which follows the DECLARATION
    // ORDER above (`Panels` is declared before `Models`). Asserting the literal
    // pair rather than a set is what keeps a reordering of that enum from
    // silently changing what every consumer reads.
    assert_eq!(
        extension.loaded_capabilities(),
        vec![Capability::Panels, Capability::Models],
        "the declarative provider and the client declaration both landed"
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
// ---------------------------------------------------------------------------
// The imperative path: the DSH host-entry bridge.
//
// These tests exercise the ONE adapter seam both standards share. The subject
// is not "did a string parse" but "is the tool CALLABLE", "was a process
// started", and "did a failure leave anything half-registered".
// ---------------------------------------------------------------------------

/// The bridge needs Node. A machine without it is a supported configuration:
/// the entry is refused by name. The tests below therefore SKIP rather than
/// fail when no interpreter exists, and the refusal itself is asserted by
/// `an_entry_without_an_interpreter_is_refused_by_name`.
fn bridge_ready() -> bool {
    agent::extensions::bridge::find_interpreter().is_some()
}

/// A scratch copy of a fixture, so a test that writes (a marker file, a
/// corrupted entry) never touches the repository.
///
/// The copy is taken from a CLEAN source: a fixture that records execution by
/// writing a marker is checked before it is copied, because a marker left behind
/// by an earlier run would make every "nothing ran" assertion below pass or fail
/// for the wrong reason. That is not hypothetical - a previous revision of this
/// suite trusted the source directory and a leaked marker silently inverted the
/// trust tests until they were read again.
fn scratch_copy(name: &str) -> PathBuf {
    let source = fixture(name);
    assert_clean_fixture(&source);
    let root = std::env::temp_dir().join(format!(
        "bos-ext-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    copy_tree(&source, &root);
    root
}

/// Refuse to start from a dirty fixture.
///
/// A marker file is evidence that a plugin RAN, so one sitting in the fixture is
/// evidence that a previous run executed something it should not have. Failing
/// here names the contamination instead of letting it silently rewrite what the
/// trust tests mean.
fn assert_clean_fixture(source: &Path) {
    for entry in std::fs::read_dir(source).expect("read fixture") {
        let entry = entry.expect("entry");
        let name = entry.file_name();
        let name = name.to_string_lossy();
        assert!(
            !name.ends_with(".marker"),
            "the fixture {} carries {name}: a previous run left execution evidence \
             behind, so every assertion about it would be read from the wrong \
             baseline. Delete it before running the suite.",
            source.display()
        );
    }
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("scratch dir");
    for entry in std::fs::read_dir(from).expect("read fixture") {
        let entry = entry.expect("entry");
        let path = entry.path();
        let target = to.join(entry.file_name());
        if path.is_dir() {
            copy_tree(&path, &target);
        } else {
            std::fs::copy(&path, &target).expect("copy fixture file");
        }
    }
}

/// Whether any process with this pid exists, without executing a new one.
///
/// `kill(pid, 0)` is the POSIX existence probe: it sends no signal and
/// reports through `errno`. It is the cheapest honest witness that the child
/// is gone, and it costs no second subprocess that could itself be confused
/// with the one under test.
#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    // SAFETY: `kill` with signal 0 performs the permission and existence
    // check and delivers nothing; the pid is a value the OS issued us.
    unsafe { kill(pid as i32, 0) == 0 }
}

#[cfg(not(unix))]
fn process_alive(pid: u32) -> bool {
    let _ = pid;
    false
}

/// A TRUSTED DSH package's host entry runs, and the tool it registers is
/// CALLABLE through the agent's own tool registry.
///
/// The assertion is the CALL, not the registration: a bridge that announced a
/// tool and could not run it would pass a registration-only test while being
/// useless. The result value is compared field for field against what the
/// fixture's `execute` returns.
#[test]
fn a_trusted_dsh_entry_registers_a_tool_that_the_agent_can_call() {
    if !bridge_ready() {
        eprintln!("skipped: no Node interpreter");
        return;
    }
    let root = scratch_copy("dsh-bridge");
    let package =
        ExtensionRegistry::load_with_trust(&root, Trust::Trusted).expect("a trusted package loads");
    let extension = &package.extension;
    assert_eq!(extension.id, "@local/dsh-bridge");
    let names = extension
        .contributions
        .iter()
        .map(|contribution| contribution.name())
        .collect::<Vec<_>>();
    assert!(
        names.contains(&"dsh_bridge_echo".to_string()),
        "the bridged tool is in the registry contributions: {names:?}"
    );
    assert!(
        names.contains(&"bridge-hello".to_string()),
        "the bridged command is in the registry contributions: {names:?}"
    );

    let mut registry = ExtensionRegistry::new();
    let session = package.session.clone();
    let live_session = package
        .session
        .clone()
        .expect("a trusted load yields a session");
    registry.install(package).expect("installs");

    let mut agent = Agent::from_config(AgentConfig::default());
    registry.apply_to_agent(&mut agent).expect("applies");

    let tool = agent
        .registry()
        .expect("tool registry")
        .get("dsh_bridge_echo")
        .expect("the bridged tool reached the agent registry");
    let output = tool
        .run(&json!({ "value": "hi" }))
        .expect("the bridged tool runs");
    assert_eq!(output["echoed"], "hi", "{output}");
    assert_eq!(output["plugin"], "fixture", "{output}");

    // The JSON Schema the plugin declared survives the round trip, so the
    // model is offered the real argument shape.
    let schema = tool.json_schema();
    assert_eq!(schema["properties"]["value"]["type"], "string");
    assert_eq!(schema["required"][0], "value");

    // The session is what keeps the plugin's process alive, and it is shared:
    // the registry holds a handle and this test's clone is a second one. So the
    // child must STILL be running here - dropping one clone of two must not kill
    // a plugin whose tools the agent is still holding.
    let pid = session
        .as_ref()
        .map(|session| {
            session
                .lock()
                .map(|session| session.pid())
                .unwrap_or_default()
        })
        .expect("a trusted load yields a session");
    assert!(
        process_alive(pid),
        "the agent still holds a bridged tool, so its process is still alive"
    );
    drop(session);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(
        process_alive(pid),
        "dropping ONE handle must not stop a plugin another handle still backs"
    );

    // The tool is still CALLABLE after that handle went away, which is the fact
    // that matters: the registry owns the session, not the caller.
    let again = tool
        .run(&json!({ "value": "second" }))
        .expect("still callable");
    assert_eq!(again["echoed"], "second", "{again}");

    // Killing the session the tool holds is the unload path, and it is observable
    // from the tool: a call after teardown fails instead of hanging.
    if let Ok(mut live) = live_session.lock() {
        live.kill();
    }
    let after = tool.run(&json!({ "value": "third" }));
    assert!(
        after.is_err(),
        "a bridged tool reports failure once its session is gone: {after:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
/// An UNTRUSTED package's entry never runs: no process, no marker, no tool.
///
/// The fixture writes a marker file on the FIRST line of its module body, i.e.
/// at import time, before `apply` is reached. So the marker is evidence of
/// execution itself and not merely of a successful `apply`. The test asserts
/// three separate facts: the marker is absent, the tool is absent, and the
/// refusal is the named one that says why.
#[test]
fn an_untrusted_dsh_entry_is_refused_and_starts_no_process() {
    let root = scratch_copy("dsh-untrusted");
    let marker = root.join("executed.marker");
    assert!(!marker.exists(), "the fixture starts clean");

    let package = ExtensionRegistry::load_with_trust(&root, Trust::Untrusted).expect("loads");
    assert!(
        !package.is_fully_supported(),
        "an untrusted imperative entry is refused"
    );
    let refusal = package
        .refusals
        .iter()
        .find(|refusal| refusal.feature == RefusedFeature::ImperativeHostEntry)
        .expect("the host entry refusal");
    let message = refusal.to_string();
    assert!(message.contains("@local/dsh-untrusted"), "{message}");
    assert!(message.contains("index.js"), "{message}");
    assert!(message.contains("JavaScript"), "{message}");

    assert!(
        !marker.exists(),
        "no process ran, so the entry never reached its first line"
    );
    assert!(
        package.session.is_none(),
        "an untrusted package holds no session and therefore no child"
    );
    assert!(
        package
            .extension
            .contributions
            .iter()
            .all(|contribution| contribution.name() != "dsh_untrusted_echo"),
        "nothing the entry would register is present"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The same package, TRUSTED, runs: the marker appears and the tool exists.
///
/// This is the discriminating half of the test above. Without it, "the marker
/// is absent" would pass for a fixture that never worked in the first place.
#[test]
fn the_same_entry_runs_once_the_reader_trusts_it() {
    if !bridge_ready() {
        eprintln!("skipped: no Node interpreter");
        return;
    }
    let root = scratch_copy("dsh-untrusted");
    let marker = root.join("executed.marker");
    let package =
        ExtensionRegistry::load_with_trust(&root, Trust::Trusted).expect("a trusted package loads");
    assert!(
        marker.exists(),
        "a trusted entry executes, which is exactly what trust buys"
    );
    assert!(
        package
            .extension
            .contributions
            .iter()
            .any(|contribution| contribution.name() == "dsh_untrusted_echo"),
        "the trusted entry's tool is registered"
    );
    drop(package);
    let _ = std::fs::remove_dir_all(&root);
}
/// A TRUSTED entry that throws produces ONE named refusal and no partial load.
///
/// The fixture registers a tool and THEN throws. If the loader kept what it had
/// collected so far, the host would trust a plugin that never finished
/// loading. The test asserts the tool that was registered before the throw is
/// absent, which is the only observable that distinguishes an all-or-nothing
/// loader from a forgiving one.
#[test]
fn a_trusted_entry_that_throws_leaves_a_named_refusal_and_no_partial_load() {
    if !bridge_ready() {
        eprintln!("skipped: no Node interpreter");
        return;
    }
    let root = scratch_copy("dsh-throwing");
    let package = ExtensionRegistry::load_with_trust(&root, Trust::Trusted)
        .expect("the package is still readable");
    assert!(
        package.session.is_none(),
        "a failed load keeps no live session"
    );
    assert!(
        package
            .extension
            .contributions
            .iter()
            .all(|contribution| contribution.name() != "dsh_before_throw"),
        "the tool registered before the throw does not survive it"
    );
    let refusal = package
        .refusals
        .iter()
        .find(|refusal| refusal.feature == RefusedFeature::ImperativeHostEntry)
        .expect("the named refusal");
    let message = refusal.to_string();
    assert!(
        message.contains("the plugin threw while loading"),
        "the refusal names the failure: {message}"
    );
    assert!(
        message.contains("refuses to load"),
        "the refusal carries the plugin's own message: {message}"
    );
    assert!(
        message.contains("Nothing it would have registered was loaded"),
        "the refusal states the all-or-nothing outcome: {message}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The bridge cannot hang: a plugin that never returns from `apply` is killed
/// at the deadline, the refusal names the timeout, and no child survives.
#[test]
fn a_hanging_entry_times_out_and_leaves_no_child_process() {
    if !bridge_ready() {
        eprintln!("skipped: no Node interpreter");
        return;
    }
    let root = scratch_copy("dsh-hanging");
    let interpreter = agent::extensions::bridge::find_interpreter().expect("an interpreter");
    let options =
        agent::extensions::bridge::BridgeOptions::new(interpreter, &root, root.join("index.js"))
            .with_load_timeout(std::time::Duration::from_millis(1500));
    let started = std::time::Instant::now();
    let error = match agent::extensions::bridge::BridgeSession::start(options) {
        Ok(_) => panic!("a hanging plugin must not load"),
        Err(error) => error,
    };
    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "the deadline is the bound, not the plugin: {elapsed:?}"
    );
    let message = error.to_string();
    assert!(message.contains("did not answer"), "{message}");
    assert!(message.contains("killed"), "{message}");
    let _ = std::fs::remove_dir_all(&root);
}

/// A trusted load is bounded and leaves the process table clean once the
/// session is dropped, proved by the pid the session reported.
#[test]
fn a_bridged_session_reports_a_pid_and_reaps_it_on_unload() {
    if !bridge_ready() {
        eprintln!("skipped: no Node interpreter");
        return;
    }
    let root = scratch_copy("dsh-bridge");
    let interpreter = agent::extensions::bridge::find_interpreter().expect("an interpreter");
    let options =
        agent::extensions::bridge::BridgeOptions::new(interpreter, &root, root.join("index.js"));
    let mut sessions = agent::extensions::bridge::BridgeRegistry::new();
    sessions.load("@local/dsh-bridge", options).expect("loads");
    assert_eq!(sessions.loaded(), vec!["@local/dsh-bridge".to_string()]);
    let pid = sessions
        .session("@local/dsh-bridge")
        .expect("a session")
        .lock()
        .expect("lock")
        .pid();
    assert!(process_alive(pid), "the child is running while loaded");
    assert!(sessions.unload("@local/dsh-bridge"));
    assert!(sessions.loaded().is_empty());
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(
        !process_alive(pid),
        "unload reaps the child rather than leaking it"
    );
    let _ = std::fs::remove_dir_all(&root);
}
// ---------------------------------------------------------------------------
// The Codex standard: what its shim loads, and what it refuses by name.
// ---------------------------------------------------------------------------

/// A Codex plugin's `interface` block is captured, not dropped.
///
/// Measured on the 64 shipped manifests, every one carries these fields and
/// the reference's plugin manager draws a card from them. Dropping them made
/// the row unable to render what the package publishes about itself.
#[test]
fn a_codex_interface_block_is_read_into_the_substrate() {
    let package = ExtensionRegistry::load(&fixture("codex-deploy")).expect("loads");
    let interface = package
        .extension
        .details
        .interface
        .clone()
        .expect("the fixture declares an interface block");
    assert_eq!(
        interface.display_name.as_deref(),
        Some("Codex Deploy Fixture")
    );
    assert_eq!(interface.category.as_deref(), Some("Developer Tools"));
    assert_eq!(interface.developer_name.as_deref(), Some("Fixture Co"));
    assert_eq!(interface.brand_color.as_deref(), Some("#0a0a0a"));
    assert_eq!(
        interface.capabilities,
        vec!["Read".to_string(), "Write".to_string()]
    );
    assert!(
        interface
            .default_prompt
            .iter()
            .any(|prompt| prompt.contains("Deploy")),
        "the suggested prompts survive: {interface:?}"
    );
}

/// A Codex `agents/*.md` file is a prompt, and its frontmatter name is the
/// contribution name - not the file stem.
#[test]
fn a_codex_agent_prompt_carries_its_frontmatter_name() {
    let registry = installed("codex-deploy");
    let reviewer = registry
        .prompts()
        .into_iter()
        .find(|prompt| prompt.name == "reviewer")
        .expect("the agent prompt");
    assert!(
        reviewer.body.contains("review changes"),
        "{}",
        reviewer.body
    );
}

/// The Codex MCP refusal names the reachable path: a host with an MCP pool
/// reads the same `.mcp.json`, so the refusal has to say so rather than
/// implying the servers are unusable on this machine.
#[test]
fn a_codex_mcp_refusal_names_the_path_that_can_start_it() {
    let package = ExtensionRegistry::load(&fixture("codex-deploy")).expect("loads");
    let refusal = package
        .refusals
        .iter()
        .find(|refusal| refusal.feature == RefusedFeature::McpServer)
        .expect("the MCP refusal");
    let message = refusal.to_string();
    assert!(message.contains("fixture-server"), "{message}");
    assert!(
        message.to_lowercase().contains("trust"),
        "the refusal points at the trust gate as the way to start it: {message}"
    );
    assert!(!message.contains("sk-fixture-secret"), "{message}");
}

/// A Codex app connector is refused with its OWN named reason: an app is not
/// an MCP server and must not borrow the MCP sentence.
#[test]
fn a_codex_app_connector_is_refused_with_its_own_reason() {
    let package = ExtensionRegistry::load(&fixture("codex-deploy")).expect("loads");
    let refusal = package
        .refusals
        .iter()
        .find(|refusal| refusal.feature == RefusedFeature::ExternalApp)
        .expect("the app refusal");
    let message = refusal.to_string();
    assert!(message.contains("fixture-app"), "{message}");
    assert!(
        !message.contains("mcp"),
        "an app connector does not borrow the MCP wording: {message}"
    );
}

/// The Codex path still loads its declarative parts - tools, commands,
/// skills and prompts - while refusing MCP and apps.
#[test]
fn the_codex_path_loads_declarative_parts_and_refuses_the_rest() {
    let package = ExtensionRegistry::load(&fixture("codex-deploy")).expect("loads");
    assert!(!package.is_fully_supported(), "it refuses something");
    let mut registry = ExtensionRegistry::new();
    registry.install(package).expect("installs");
    assert!(!registry.tool_names().is_empty(), "a tool loaded");
    assert!(!registry.commands().is_empty(), "a command loaded");
    assert!(!registry.skills().is_empty(), "a skill loaded");
    assert!(!registry.prompts().is_empty(), "a prompt loaded");
    assert!(
        registry
            .refusals()
            .iter()
            .any(|refusal| refusal.feature == RefusedFeature::McpServer),
        "the MCP declaration is on the refusal list"
    );
    assert!(
        registry
            .refusals()
            .iter()
            .any(|refusal| refusal.feature == RefusedFeature::ExternalApp),
        "the app connector is on the refusal list"
    );
}
/// A plugin that reaches an unimplemented host service fails LOUDLY and by
/// name, rather than loading half of what it declares.
///
/// This is the shape of a real package: `@opencode2dsh/dsh-plugin` opens with
/// `ctx.llm.registerAdapter([id], adapter)`, and a bridge that stubbed
/// `ctx.llm` as an empty object would let the plugin believe it had published
/// models. The refusing seam turns that into one named refusal and zero
/// registrations.
#[test]
fn an_unimplemented_host_service_is_refused_by_name_and_loads_nothing() {
    if !bridge_ready() {
        eprintln!("skipped: no Node interpreter");
        return;
    }
    let scratch = tempfile::tempdir().expect("tempdir");
    let entry = scratch.path().join("index.js");
    std::fs::write(
        &entry,
        concat!(
            "module.exports = { apply(ctx) {\n",
            "  ctx.llm.registerAdapter(['zen'], {});\n",
            "} };\n"
        ),
    )
    .expect("write the entry");
    let interpreter = agent::extensions::bridge::find_interpreter().expect("an interpreter");
    let options =
        agent::extensions::bridge::BridgeOptions::new(interpreter, scratch.path(), &entry);
    let error = match agent::extensions::bridge::BridgeSession::start(options) {
        Ok(_) => panic!("an unimplemented service must fail the load, not pass silently"),
        Err(error) => error,
    };
    let message = error.to_string();
    assert!(message.contains("ctx.llm"), "{message}");
    assert!(
        message.to_lowercase().contains("model"),
        "the refusal says which seam and what this host offers instead: {message}"
    );
    let named = agent::extensions::bridge::refusal_for("@local/zen", &entry, &error);
    let sentence = named.to_string();
    assert!(
        sentence.contains("Nothing it would have registered was loaded"),
        "{sentence}"
    );
}
/// A packed Codex plugin ships `*.md.tmpl` twins of its commands and `_`-prefixed
/// shared fragments. Both were mishandled before: the template was dropped
/// entirely (half the package's commands vanished) and the fragment was
/// registered as a reader-facing command.
///
/// The fixture carries all three shapes - `deploy.md`, `deploy.md.tmpl` and
/// `_conventions.md` - so one test can assert the whole rule: both deploy
/// files reduce to ONE command named `deploy`, and `_conventions` is absent.
#[test]
fn a_packed_codex_command_template_is_read_and_a_fragment_is_not_a_command() {
    let registry = installed("codex-deploy");
    let names: Vec<&str> = registry
        .commands()
        .iter()
        .map(|command| command.name.as_str())
        .collect();
    assert!(
        names.contains(&"deploy"),
        "the command is registered under its plain name: {names:?}"
    );
    assert!(
        !names.iter().any(|name| name.contains("tmpl")),
        "the packer's .tmpl marker never reaches a command name: {names:?}"
    );
    assert_eq!(
        names.iter().filter(|name| **name == "deploy").count(),
        1,
        "deploy.md and deploy.md.tmpl are ONE command, not two: {names:?}"
    );
    assert!(
        !names.iter().any(|name| name.starts_with('_')),
        "a _-prefixed fragment is a private helper, not a command: {names:?}"
    );
    // The prompt side follows the same rule, so the two surfaces cannot
    // disagree about what the package ships.
    let prompts: Vec<&str> = registry
        .prompts()
        .iter()
        .map(|prompt| prompt.name.as_str())
        .collect();
    assert!(prompts.contains(&"deploy"), "{prompts:?}");
    assert!(
        !prompts.iter().any(|name| name.starts_with('_')),
        "{prompts:?}"
    );
}

/// A plugin-level `agents/*.md` is a subagent prompt, named from its
/// frontmatter, and it does not shadow the command of the same stem.
#[test]
fn a_packed_codex_agent_file_is_a_prompt_not_a_command() {
    let registry = installed("codex-deploy");
    let prompts: Vec<&str> = registry
        .prompts()
        .iter()
        .map(|prompt| prompt.name.as_str())
        .collect();
    assert!(
        prompts.contains(&"reviewer-agent"),
        "the agent prompt is named from its frontmatter: {prompts:?}"
    );
    assert!(
        !registry
            .commands()
            .iter()
            .any(|command| command.name == "reviewer-agent"),
        "an agent file is not a host command"
    );
}

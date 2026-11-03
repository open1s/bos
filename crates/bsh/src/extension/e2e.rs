//! End-to-end extension tests from BSH's own side.
//!
//! These use the real example packages and the real host/store/policy/market;
//! they do not call runtime internals directly unless the production path does.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use super::host::{ExtensionHost, HostConfig};
use super::market::GitMarket;
use ferris_sdk::capability::{GrantScope, PolicyEngine, PolicyMode};
use ferris_sdk::package::ExtensionStore;
use ferris_sdk::types::ExtensionId;

static SEQUENCE: AtomicUsize = AtomicUsize::new(0);

fn scratch(label: &str) -> PathBuf {
    let index = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "bsh-ext-e2e-{label}-{}-{index}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("scratch");
    root
}

fn examples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/extensions")
}

fn config(root: &Path) -> HostConfig {
    HostConfig {
        root: root.to_path_buf(),
        workspace: Some(root.join("workspace")),
        disabled: std::collections::BTreeSet::new(),
        preset_allowed: None,
    }
}

fn grant_all(
    store: &ExtensionStore,
    id: &ExtensionId,
    required: &[ferris_sdk::capability::Capability],
) {
    let policy = PolicyEngine::open(store.grants_path(), store.audit_path(), PolicyMode::Enforce)
        .expect("policy");
    for capability in required {
        policy
            .grant(id.as_str(), *capability, GrantScope::Any, None)
            .expect("grant");
    }
}

#[tokio::test]
async fn the_declarative_example_applies_all_file_contributions() {
    let root = scratch("declarative");
    std::fs::create_dir_all(root.join("workspace")).expect("workspace");
    let store = ExtensionStore::open(&root).expect("store");
    let package = store.install(&examples().join("hello")).expect("install");
    grant_all(
        &store,
        package.id(),
        &package.manifest.capabilities.required,
    );
    let mut host = ExtensionHost::open(config(&root)).expect("host");
    let status = host
        .statuses()
        .into_iter()
        .find(|status| status.id == "com.example.hello")
        .expect("hello status");
    assert!(status.applied, "hello applies: {:?}", status.error);
    assert!(!status.tools.is_empty(), "hello contributes a tool");
    assert!(!status.commands.is_empty(), "hello contributes a command");
    assert!(!status.panels.is_empty(), "hello contributes a panel");
    host.shutdown().await;
}

#[tokio::test]
async fn a_git_market_install_reaches_the_installed_inventory() {
    let root = scratch("market");
    std::fs::create_dir_all(root.join("workspace")).expect("workspace");
    // Seed the cache a successful `bsh ext market` sync leaves behind. The
    // clone itself is covered by the CLI run; forking `git` from a
    // multithreaded test process is unsafe on macOS.
    let provider = root.join("market/provider");
    std::fs::create_dir_all(provider.join("packages")).expect("provider");
    copy_dir(&examples().join("hello"), &provider.join("packages/hello"));
    std::fs::write(
        provider.join("index.json"),
        r#"{"schema_version":1,"extensions":[{"id":"com.example.hello","name":"Hello","version":"1.0.0","description":"hello","path":"packages/hello"}]}"#,
    )
    .expect("index");

    let store = ExtensionStore::open(&root).expect("store");
    let market = GitMarket::open(&root, "https://example.invalid/provider");
    let index = market.cached().expect("cached index");
    let entry = index.newest("com.example.hello").expect("entry");
    let package = market.install(&store, entry).expect("install");
    grant_all(
        &store,
        package.id(),
        &package.manifest.capabilities.required,
    );

    let mut host = ExtensionHost::open(config(&root)).expect("host");
    let status = host
        .statuses()
        .into_iter()
        .find(|status| status.id == "com.example.hello")
        .expect("installed status");
    assert!(status.applied, "market package applies: {:?}", status.error);
    assert_eq!(status.trust, "community");
    assert!(status.tools.iter().any(|tool| tool.ends_with("__echo")));
    host.shutdown().await;
}

#[tokio::test]
async fn the_hook_demo_language_integration_and_hooks_are_live() {
    let root = scratch("hooks");
    std::fs::create_dir_all(root.join("workspace")).expect("workspace");
    let store = ExtensionStore::open(&root).expect("store");
    let package = store
        .install(&examples().join("hook-demo"))
        .expect("install");
    grant_all(
        &store,
        package.id(),
        &package.manifest.capabilities.required,
    );
    let mut host = ExtensionHost::open(config(&root)).expect("host");
    let document = root.join("main.py");
    std::fs::write(&document, "def ok():\n    return 1\n").expect("document");
    let diagnostics = host
        .language_check("com.example.hook-demo", "hookdemo", &document)
        .await
        .expect("language check");
    assert_eq!(diagnostics["arguments"]["language"], "hookdemo");

    let event = ferris_sdk::events::ExtensionEvent::new(
        "bsh",
        ferris_sdk::events::topics::SESSION_CREATED,
        serde_json::json!({"session": "e2e"}),
    )
    .expect("event");
    let mut log = serde_json::Value::Null;
    for _ in 0..40 {
        host.events().publish(&event).expect("publish");
        log = host
            .run_integration("com.example.hook-demo", "github", serde_json::json!({}))
            .await
            .expect("integration tool");
        if log["events"].as_array().is_some_and(|events| {
            events
                .iter()
                .any(|event| event["topic"] == "session.created")
        }) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        log["events"].as_array().is_some_and(|events| events
            .iter()
            .any(|event| event["topic"] == "session.created")),
        "the process never saw session.created: {log}"
    );
    let command = host
        .run_integration("com.example.hook-demo", "command", serde_json::json!({}))
        .await
        .expect("integration command");
    assert_eq!(command["id"], "events");

    host.shutdown().await;
    assert_eq!(host.events().subscriber_count(), 0);
}

#[tokio::test]
async fn the_computer_use_example_exposes_every_declared_surface() {
    let root = scratch("computer-use");
    std::fs::create_dir_all(root.join("workspace")).expect("workspace");
    let store = ExtensionStore::open(&root).expect("store");
    let package = store
        .install(&examples().join("computer-use"))
        .expect("install");
    grant_all(
        &store,
        package.id(),
        &package.manifest.capabilities.required,
    );
    let mut host = ExtensionHost::open(config(&root)).expect("host");
    let status = host
        .statuses()
        .into_iter()
        .find(|status| status.id == "com.example.computer-use")
        .expect("computer-use status");
    assert!(status.applied, "computer-use applies: {:?}", status.error);
    assert!(status
        .tools
        .iter()
        .any(|tool| tool.ends_with("__screen_size")));
    assert!(status.tools.iter().any(|tool| tool.ends_with("__inspect")));
    assert!(status
        .commands
        .iter()
        .any(|command| command.ends_with("__refresh")));
    assert_eq!(status.panels, ["computer-use__desktop"]);
    assert_eq!(host.integrations().len(), 2);
    host.shutdown().await;
}

#[tokio::test]
async fn the_cordis_example_runs_through_the_host() {
    let root = scratch("cordis");
    std::fs::create_dir_all(root.join("workspace")).expect("workspace");
    let store = ExtensionStore::open(&root).expect("store");
    let package = store
        .install(&examples().join("cordis-echo"))
        .expect("install");
    grant_all(
        &store,
        package.id(),
        &package.manifest.capabilities.required,
    );
    let mut host = ExtensionHost::open(config(&root)).expect("host");
    let status = host
        .statuses()
        .into_iter()
        .find(|status| status.id == "com.example.cordis-echo")
        .expect("cordis status");
    assert!(status.applied, "cordis applies: {:?}", status.error);
    assert_eq!(status.runtime, "cordis");
    assert!(
        status.tools.iter().any(|tool| tool.ends_with("__echo")),
        "tools: {:?}",
        status.tools
    );
    assert!(
        status
            .commands
            .iter()
            .any(|command| command.ends_with("__hello")),
        "commands: {:?}",
        status.commands
    );
    assert!(
        status.skills.iter().any(|skill| skill == "greet"),
        "skills: {:?}",
        status.skills
    );
    host.shutdown().await;
}

#[tokio::test]
async fn a_profile_mount_applies_config_to_an_installed_package() {
    let root = scratch("mount");
    std::fs::create_dir_all(root.join("workspace")).expect("workspace");
    let store = ExtensionStore::open(&root).expect("store");
    let package = store.install(&examples().join("hello")).expect("install");
    grant_all(
        &store,
        package.id(),
        &package.manifest.capabilities.required,
    );
    let mut host = ExtensionHost::open(config(&root)).expect("host");
    let profile = root.join("profile.yml");
    std::fs::write(
        &profile,
        "- insert:\n    - id: com.example.hello\n      name: hello\n      config:\n        greeting: hi\n",
    )
    .expect("profile");
    let mounted = host.mount_profile(&profile).expect("mount");
    assert_eq!(mounted, vec!["com.example.hello".to_string()]);
    assert!(host
        .statuses()
        .into_iter()
        .any(|status| status.id == "com.example.hello" && status.applied));
    host.shutdown().await;
}

#[tokio::test]
async fn the_first_party_notes_package_installs_through_the_store() {
    let root = scratch("bsh-notes");
    std::fs::create_dir_all(root.join("workspace")).expect("workspace");
    let store = ExtensionStore::open(&root).expect("store");
    let package = store
        .install(&examples().join("bsh-notes"))
        .expect("install");
    grant_all(
        &store,
        package.id(),
        &package.manifest.capabilities.required,
    );
    let mut host = ExtensionHost::open(config(&root)).expect("host");
    let status = host
        .statuses()
        .into_iter()
        .find(|status| status.id == "com.bos.bsh.notes")
        .expect("notes status");
    assert!(status.applied, "notes applies: {:?}", status.error);
    assert_eq!(status.runtime, "process");
    assert!(status.tools.iter().any(|tool| tool.ends_with("__save")));
    assert!(status.tools.iter().any(|tool| tool.ends_with("__list")));
    host.shutdown().await;
}

#[tokio::test]
async fn disabling_the_first_party_notes_package_removes_its_tools() {
    let root = scratch("bsh-notes-disabled");
    std::fs::create_dir_all(root.join("workspace")).expect("workspace");
    let store = ExtensionStore::open(&root).expect("store");
    let package = store
        .install(&examples().join("bsh-notes"))
        .expect("install");
    grant_all(
        &store,
        package.id(),
        &package.manifest.capabilities.required,
    );
    store
        .set_enabled(package.id(), false)
        .expect("disable notes");
    let mut host = ExtensionHost::open(config(&root)).expect("host");
    let status = host
        .statuses()
        .into_iter()
        .find(|status| status.id == "com.bos.bsh.notes")
        .expect("notes status");
    assert!(!status.configured);
    assert!(!status.applied);
    host.shutdown().await;
}

#[tokio::test]
async fn a_live_suspend_hides_commands_and_panels() {
    let root = scratch("live-commands");
    std::fs::create_dir_all(root.join("workspace")).expect("workspace");
    let store = ExtensionStore::open(&root).expect("store");
    let package = store.install(&examples().join("hello")).expect("install");
    grant_all(
        &store,
        package.id(),
        &package.manifest.capabilities.required,
    );
    let mut host = ExtensionHost::open(config(&root)).expect("host");
    let id = package.id().as_str().to_string();
    let commands = host.command_surfaces().len();
    let panels = host.panel_surfaces().len();
    assert!(commands > 0, "hello contributes a command");
    assert!(panels > 0, "hello contributes a panel");
    crate::extension::live::set_suspended(&id, true);
    assert!(host.command_surfaces().len() < commands);
    assert!(host.panel_surfaces().len() < panels);
    crate::extension::live::set_suspended(&id, false);
    assert_eq!(host.command_surfaces().len(), commands);
    assert_eq!(host.panel_surfaces().len(), panels);
    host.shutdown().await;
}

#[tokio::test]
async fn disabling_the_annotations_package_disables_the_notes_store() {
    let root = scratch("annotations-provider");
    std::fs::create_dir_all(root.join("workspace")).expect("workspace");
    let mut host = ExtensionHost::open(config(&root)).expect("host");
    let id = ferris_sdk::types::ExtensionId::parse("com.bos.bsh.annotations").expect("id");
    let status = host
        .statuses()
        .into_iter()
        .find(|status| status.id == "com.bos.bsh.annotations")
        .expect("annotations status");
    assert!(status.applied, "annotations provider applies");
    let mut notes = crate::notes::Notes::default();
    notes.add(
        std::path::Path::new("a.txt"),
        crate::notes::Note {
            line: None,
            to: None,
            text: "hi".into(),
        },
    );
    notes.save(&root).expect("save through provider");
    host.shutdown().await;

    let store = ExtensionStore::open(&root).expect("store");
    store.set_enabled(&id, false).expect("disable");
    let mut host = ExtensionHost::open(config(&root)).expect("host");
    assert!(crate::notes::Notes::load(&root)
        .on(std::path::Path::new("a.txt"))
        .is_empty());
    assert!(notes.save(&root).is_err());
    host.shutdown().await;
}

#[tokio::test]
async fn the_allinone_example_covers_every_declared_contribution_kind() {
    let root = scratch("allinone");
    std::fs::create_dir_all(root.join("workspace")).expect("workspace");
    let store = ExtensionStore::open(&root).expect("store");
    let package = store
        .install(&examples().join("allinone"))
        .expect("install");
    grant_all(
        &store,
        package.id(),
        &package.manifest.capabilities.required,
    );
    let mut host = ExtensionHost::open(config(&root)).expect("host");
    let status = host
        .statuses()
        .into_iter()
        .find(|status| status.id == "com.example.allinone")
        .expect("allinone status");
    assert!(status.applied, "allinone applies: {:?}", status.error);
    assert_eq!(status.runtime, "process");
    assert!(
        status.tools.iter().any(|tool| tool.ends_with("__describe")),
        "tools: {:?}",
        status.tools
    );
    assert!(
        !status.commands.is_empty(),
        "commands: {:?}",
        status.commands
    );
    assert!(!status.panels.is_empty(), "panels: {:?}", status.panels);
    assert!(!status.skills.is_empty(), "skills: {:?}", status.skills);
    assert!(
        !status.languages.is_empty(),
        "languages: {:?}",
        status.languages
    );
    assert!(
        !status.integrations.is_empty(),
        "integrations: {:?}",
        status.integrations
    );

    // An active package's SKILL.md is only useful to the model when it reaches
    // the model-facing surface: the catalogue is rendered from
    // Agent::model_skills, and load_skill answers from Agent::all_skills. A
    // host status row proves the package parses, not that the model can see it.
    let mut agent = ::agent::Agent::from_config(::agent::AgentConfig::default());
    let mut registry = ::agent::plugins::PluginRegistry::default();
    let applied = host.apply_with_fibers(&mut agent, &mut registry, None, "session");
    assert!(
        !applied.is_empty(),
        "the allinone package activates on a real agent build"
    );
    let catalogue: Vec<String> = agent
        .model_skills()
        .into_iter()
        .map(|skill| skill.name)
        .collect();
    assert!(
        catalogue.iter().any(|name| name == "allinone"),
        "the package's skill reaches the model catalogue: {catalogue:?}"
    );
    assert!(
        agent
            .all_skills()
            .iter()
            .any(|skill| skill.metadata.name == "allinone"),
        "and the loader can open the same skill"
    );

    host.shutdown().await;
}

#[tokio::test]
async fn the_grant_button_path_activates_a_denied_community_package() {
    let root = scratch("grant-required");
    std::fs::create_dir_all(root.join("workspace")).expect("workspace");
    let store = ExtensionStore::open(&root).expect("store");
    store
        .install(&examples().join("computer-use"))
        .expect("install");
    let mut host = ExtensionHost::open(config(&root)).expect("host");
    let id = "com.example.computer-use";
    let before = host
        .statuses()
        .into_iter()
        .find(|status| status.id == id)
        .expect("status");
    assert!(!before.applied, "critical capabilities start denied");
    let granted = host.grant_required(id).expect("grant required");
    assert!(granted >= 2, "granted {granted} capabilities");
    let after = host
        .statuses()
        .into_iter()
        .find(|status| status.id == id)
        .expect("status");
    assert!(
        after.applied,
        "granted package activates: {:?}",
        after.error
    );
    host.shutdown().await;
}

fn copy_dir(source: &Path, target: &Path) {
    std::fs::create_dir_all(target).expect("target");
    for entry in std::fs::read_dir(source).expect("source") {
        let entry = entry.expect("entry");
        let path = entry.path();
        let destination = target.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &destination);
        } else {
            std::fs::copy(&path, &destination).expect("copy");
        }
    }
}

//! End-to-end extension tests from BSH's own side.
//!
//! These use the real example packages and the real host/store/policy/market;
//! they do not call runtime internals directly unless the production path does.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use super::capability::{GrantScope, PolicyEngine, PolicyMode};
use super::host::{ExtensionHost, HostConfig};
use super::market::GitMarket;
use super::package::ExtensionStore;
use super::types::ExtensionId;

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

fn grant_all(store: &ExtensionStore, id: &ExtensionId, required: &[super::capability::Capability]) {
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

    let event = super::events::ExtensionEvent::new(
        "bsh",
        super::events::topics::SESSION_CREATED,
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

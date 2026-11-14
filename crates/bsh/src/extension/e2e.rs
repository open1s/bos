//! End-to-end extension tests from BSH's own side.
//!
//! These use the real example packages and the real host/store/policy/market;
//! they do not call runtime internals directly unless the production path does.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use super::host::{AppliedExtension, ExtensionHost, HostConfig};
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
    // Seed the cache a successful `bsh ext market` sync leaves behind: a real
    // checkout, because the install compares the index hash with the commit,
    // not with whatever the disk holds.
    // The cache directory is derived from the validated repository identity
    // (issue #305), so seed where GitMarket actually looks.
    let provider = GitMarket::open(&root, "https://example.invalid/provider")
        .path()
        .to_path_buf();
    std::fs::create_dir_all(provider.join("packages")).expect("provider");
    copy_dir(&examples().join("hello"), &provider.join("packages/hello"));
    std::fs::write(
        provider.join("index.json"),
        r#"{"schema_version":1,"extensions":[{"id":"com.example.hello","name":"Hello","version":"1.0.0","description":"hello","path":"packages/hello"}]}"#,
    )
    .expect("index");
    for args in [
        vec!["init", "-q"],
        vec!["add", "."],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "seed",
        ],
    ] {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&provider)
            .args(&args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &root)
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?} failed");
    }

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

#[cfg_attr(
    windows,
    ignore = "cordis bridge cannot resolve a drive-letter entry on Windows"
)]
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

/// Install the first-party change-history package into a scratch host and
/// return its model-facing tool through the production agent path. The
/// activation handles must outlive the tool: dropping them revokes it.
fn change_history_tool(
    root: &Path,
) -> (
    ExtensionHost,
    Vec<AppliedExtension>,
    std::sync::Arc<dyn react::tool::registry::AsyncTool>,
) {
    let store = ExtensionStore::open(root).expect("store");
    let package = store
        .install(&examples().join("change-history"))
        .expect("install");
    grant_all(
        &store,
        package.id(),
        &package.manifest.capabilities.required,
    );
    let host = ExtensionHost::open(config(root)).expect("host");
    let status = host
        .statuses()
        .into_iter()
        .find(|status| status.id == "com.bos.bsh.change-history")
        .expect("change-history status");
    assert!(status.applied, "change-history applies: {:?}", status.error);
    assert_eq!(status.runtime, "cordis");
    let qualified = status
        .tools
        .iter()
        .find(|tool| tool.ends_with("__change_history"))
        .cloned()
        .expect("the tool is registered");
    let mut agent = ::agent::Agent::from_config(::agent::AgentConfig::default());
    let mut registry = ::agent::plugins::PluginRegistry::default();
    let applied = host.apply_with_fibers(&mut agent, &mut registry, None, "session");
    let tool = agent
        .live_tools()
        .read()
        .expect("live tools")
        .get_async(&qualified)
        .expect("the tool reaches the agent");
    (host, applied, tool)
}

/// Run one change_history call against the live tool.
async fn change_history_call(
    tool: &std::sync::Arc<dyn react::tool::registry::AsyncTool>,
    arguments: serde_json::Value,
) -> Result<serde_json::Value, agent::tools::ToolError> {
    react::tool::registry::AsyncTool::run(&**tool, &arguments).await
}

/// The first-party change-history package installs through the store and
/// manages a real git repository through the production agent tool path.
#[cfg_attr(
    windows,
    ignore = "cordis bridge cannot resolve a drive-letter entry on Windows"
)]
#[tokio::test]
async fn the_change_history_package_manages_a_real_git_repository() {
    if ferris_sdk::host_runtime::bridge::find_interpreter().is_none() {
        eprintln!("skipping change-history e2e: no Node interpreter for the cordis bridge");
        return;
    }
    let root = scratch("change-history-git");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    for args in [
        vec!["init", "-q"],
        vec!["symbolic-ref", "HEAD", "refs/heads/main"],
        vec!["config", "user.name", "E2E"],
        vec!["config", "user.email", "e2e@example.invalid"],
        vec!["config", "commit.gpgsign", "false"],
    ] {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&workspace)
            .args(&args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &root)
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?} failed");
    }
    std::fs::write(workspace.join("README.md"), "seed\n").expect("seed file");
    for args in [vec!["add", "-A"], vec!["commit", "-qm", "first commit"]] {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&workspace)
            .args(&args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &root)
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?} failed");
    }

    let (mut host, _applied, tool) = change_history_tool(&root);
    let ws = workspace.display().to_string();

    // status reads the seeded repository through the terminal seam.
    let out = change_history_call(&tool, serde_json::json!({"action": "status", "path": ws}))
        .await
        .expect("status runs");
    assert_eq!(out["backend"], "git");
    assert_eq!(out["data"]["branch"], "main");

    // new seals a working-tree change into a commit.
    std::fs::write(workspace.join("change.txt"), "change\n").expect("write");
    let out = change_history_call(
        &tool,
        serde_json::json!({"action": "new", "path": ws, "message": "e2e: second commit"}),
    )
    .await
    .expect("new runs");
    assert_eq!(out["data"]["entries"][0]["subject"], "e2e: second commit");

    // log lists both commits, newest first.
    let out = change_history_call(
        &tool,
        serde_json::json!({"action": "log", "path": ws, "limit": 5}),
    )
    .await
    .expect("log runs");
    let entries = out["data"]["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 2, "{out}");
    assert_eq!(entries[1]["subject"], "first commit");

    // describe rewords HEAD.
    change_history_call(
        &tool,
        serde_json::json!({"action": "describe", "path": ws, "message": "e2e: reworded"}),
    )
    .await
    .expect("describe runs");
    let out = change_history_call(
        &tool,
        serde_json::json!({"action": "log", "path": ws, "limit": 1}),
    )
    .await
    .expect("log runs");
    assert_eq!(out["data"]["entries"][0]["subject"], "e2e: reworded");

    // undo is an honest refusal on the git backend.
    let error = change_history_call(&tool, serde_json::json!({"action": "undo", "path": ws}))
        .await
        .expect_err("git undo refuses");
    assert!(error.to_string().contains("operation log"), "{error}");

    host.shutdown().await;
}

/// The package's Git panel serves a live card through the production panel
/// seam, and the card's commit action seals a real change.
#[cfg_attr(
    windows,
    ignore = "cordis bridge cannot resolve a drive-letter entry on Windows"
)]
#[tokio::test]
async fn the_change_history_panel_serves_a_live_card_and_commits() {
    if ferris_sdk::host_runtime::bridge::find_interpreter().is_none() {
        eprintln!("skipping change-history panel e2e: no Node interpreter");
        return;
    }
    let root = scratch("change-history-panel");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    for args in [
        vec!["init", "-q"],
        vec!["symbolic-ref", "HEAD", "refs/heads/main"],
        vec!["config", "user.name", "E2E"],
        vec!["config", "user.email", "e2e@example.invalid"],
        vec!["config", "commit.gpgsign", "false"],
    ] {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&workspace)
            .args(&args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &root)
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?} failed");
    }
    std::fs::write(workspace.join("README.md"), "seed\n").expect("seed file");
    for args in [vec!["add", "-A"], vec!["commit", "-qm", "first commit"]] {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&workspace)
            .args(&args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &root)
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?} failed");
    }

    let store = ExtensionStore::open(&root).expect("store");
    let package = store
        .install(&examples().join("change-history"))
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
        .find(|status| status.id == "com.bos.bsh.change-history")
        .expect("change-history status");
    assert!(status.applied, "change-history applies: {:?}", status.error);
    assert!(
        status.panels.iter().any(|panel| panel.ends_with("__git")),
        "the Git panel is declared: {:?}",
        status.panels
    );

    // The live card comes from the runtime's panel seam, exactly as the UI
    // fetches it, and validates against the host's UI contract.
    let surface = host
        .live_panel("change-history__git")
        .expect("the panel surface");
    let value = surface.runtime.panel("git").await.expect("live card");
    let document: ferris_sdk::ui::UiDocument =
        serde_json::from_value(value.clone()).expect("card is a UiDocument");
    let report = document.validate(&package.id().clone());
    assert!(report.is_valid(), "card validates: {:?}", report.errors);
    let text = serde_json::to_string(&value).expect("card text");
    assert!(text.contains("commit"), "the message field acts: {text}");
    assert!(
        text.contains("first commit"),
        "the graph shows history: {text}"
    );

    // The card's commit action seals a working-tree change.
    std::fs::write(workspace.join("panel.txt"), "panel\n").expect("write");
    let context = ferris_sdk::host_runtime::CallContext::new(
        package.id().clone(),
        "panel:e2e",
        Some(workspace.clone()),
        30_000,
    );
    let card = surface
        .runtime
        .call_tool(
            "commit",
            serde_json::json!({"value": "panel: committed"}),
            &context,
        )
        .await
        .expect("commit action");
    let card_text = serde_json::to_string(&card).expect("card text");
    assert!(
        card_text.contains("No changes in the working copy"),
        "the refreshed card is clean: {card_text}"
    );
    let log = std::process::Command::new("git")
        .arg("-C")
        .arg(&workspace)
        .args(["log", "-1", "--format=%s"])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", &root)
        .output()
        .expect("git runs");
    assert_eq!(
        String::from_utf8_lossy(&log.stdout).trim(),
        "panel: committed"
    );

    host.shutdown().await;
}

/// The same package prefers jj when the workspace is a jj repository, and the
/// stack operations (new, squash, undo) run through the production path.
#[cfg_attr(
    windows,
    ignore = "cordis bridge cannot resolve a drive-letter entry on Windows"
)]
#[tokio::test]
async fn the_change_history_package_prefers_jj_when_present() {
    if ferris_sdk::host_runtime::bridge::find_interpreter().is_none() {
        eprintln!("skipping change-history e2e: no Node interpreter for the cordis bridge");
        return;
    }
    let jj_present = std::process::Command::new("jj")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    if !jj_present {
        eprintln!("skipping change-history jj e2e: jj is not installed");
        return;
    }
    let root = scratch("change-history-jj");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&root).expect("root");
    let init = std::process::Command::new("jj")
        .args(["git", "init", "workspace"])
        .current_dir(&root)
        .env("HOME", &root)
        .output()
        .expect("jj runs");
    assert!(
        init.status.success(),
        "jj git init: {}",
        String::from_utf8_lossy(&init.stderr)
    );
    for [key, value] in [["user.name", "E2E"], ["user.email", "e2e@example.invalid"]] {
        let config = std::process::Command::new("jj")
            .args(["config", "set", "--repo", key, value])
            .current_dir(&workspace)
            .env("HOME", &root)
            .output()
            .expect("jj runs");
        assert!(
            config.status.success(),
            "jj config: {}",
            String::from_utf8_lossy(&config.stderr)
        );
    }

    let (mut host, _applied, tool) = change_history_tool(&root);
    let ws = workspace.display().to_string();

    // status detects the jj backend.
    let out = change_history_call(&tool, serde_json::json!({"action": "status", "path": ws}))
        .await
        .expect("status runs");
    assert_eq!(out["backend"], "jj");

    // describe the working copy, then start a new change on top.
    std::fs::write(workspace.join("a.txt"), "one\n").expect("write");
    change_history_call(
        &tool,
        serde_json::json!({"action": "describe", "path": ws, "message": "e2e: first"}),
    )
    .await
    .expect("describe runs");
    let out = change_history_call(
        &tool,
        serde_json::json!({"action": "new", "path": ws, "message": "e2e: second"}),
    )
    .await
    .expect("new runs");
    assert_eq!(out["data"]["entries"][0]["description"], "e2e: second");

    // log shows the stack, newest first.
    let out = change_history_call(
        &tool,
        serde_json::json!({"action": "log", "path": ws, "limit": 5}),
    )
    .await
    .expect("log runs");
    let entries = out["data"]["entries"].as_array().expect("entries");
    assert_eq!(entries[0]["description"], "e2e: second");
    assert_eq!(entries[1]["description"], "e2e: first");

    // squash folds the working copy into its parent without opening an editor.
    std::fs::write(workspace.join("b.txt"), "two\n").expect("write");
    change_history_call(&tool, serde_json::json!({"action": "squash", "path": ws}))
        .await
        .expect("squash runs");
    let out = change_history_call(
        &tool,
        serde_json::json!({"action": "log", "path": ws, "limit": 3}),
    )
    .await
    .expect("log runs");
    let entries = out["data"]["entries"].as_array().expect("entries");
    assert_eq!(entries[1]["description"], "e2e: first");

    // undo reverses the last operation.
    let out = change_history_call(&tool, serde_json::json!({"action": "undo", "path": ws}))
        .await
        .expect("undo runs");
    let text = format!(
        "{}{}",
        out["output"].as_str().unwrap_or(""),
        out["stderr"].as_str().unwrap_or("")
    );
    assert!(text.contains("Undid operation"), "{text}");

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
        crate::notes::Note::new(None, None, "hi", ""),
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

/// A Cordis package registers its skill at ACTIVATION, not on disk. The
/// runtime materializes it into its data directory, and the host has to read
/// that directory: the file-based `skills/<id>/SKILL.md` form is only one way a
/// package can contribute a skill.
#[cfg_attr(
    windows,
    ignore = "cordis bridge cannot resolve a drive-letter entry on Windows"
)]
#[tokio::test]
async fn a_runtime_registered_skill_reaches_the_agent() {
    let root = scratch("cordis-runtime-skill");
    std::fs::create_dir_all(root.join("workspace")).expect("workspace");
    let source = root.join("source");
    std::fs::create_dir_all(&source).expect("source");
    std::fs::write(
        source.join("extension.toml"),
        r#"
schema_version = "2.0"
[extension]
id = "com.example.runtime-skill"
name = "Runtime Skill"
version = "1.0.0"
description = "registers its skill at activation"
[compatibility]
host = ">=3.0.0, <4.0.0"
extension_api = "^1.0"
[runtime]
kind = "cordis"
[runtime.config]
entry = "index.js"
startup = "on-demand"
[capabilities]
required = ["process.execute", "agent.tools.register", "agent.skills.provide"]
[[contribution]]
kind = "tool"
id = "echo"
[contribution.payload]
"#,
    )
    .expect("manifest");
    std::fs::write(
        source.join("index.js"),
        r##"
module.exports = {
  apply(ctx) {
    ctx.tools.register({
      name: "echo",
      description: "Echo a value.",
      parameters: { type: "object", properties: { value: { type: "string" } } },
      output: { schema: {}, render: () => null },
      async execute(args) { return { echoed: args.value }; },
    });
    ctx.skills.register({
      name: "runtime-tour",
      description: "a tour registered at activation",
      content: "# Runtime Tour\n\nRUNTIME TOUR BODY",
    });
  },
};
"##,
    )
    .expect("entry");

    let store = ExtensionStore::open(&root).expect("store");
    let package = store.install(&source).expect("install");
    grant_all(
        &store,
        package.id(),
        &package.manifest.capabilities.required,
    );
    let mut host = ExtensionHost::open(config(&root)).expect("host");
    let status = host
        .statuses()
        .into_iter()
        .find(|status| status.id == "com.example.runtime-skill")
        .expect("status");
    assert!(status.applied, "the package activates: {:?}", status.error);
    assert!(
        status.skills.iter().any(|skill| skill == "runtime-tour"),
        "the status row names the runtime-registered skill: {:?}",
        status.skills
    );

    let saw = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut provider = ::agent::agent::agentic::LlmProvider::new();
    provider.register_vendor(
        "mock".to_string(),
        Box::new(SkillLoader {
            skill: "runtime-tour",
            body: "RUNTIME TOUR BODY",
            calls: AtomicUsize::new(0),
            saw: std::sync::Arc::clone(&saw),
        }),
    );
    let mut agent = ::agent::Agent::new(
        ::agent::AgentConfig {
            model: "mock-model".to_string(),
            ..Default::default()
        },
        std::sync::Arc::new(provider),
    );
    let mut registry = ::agent::plugins::PluginRegistry::default();
    let applied = host.apply_with_fibers(&mut agent, &mut registry, None, "session");
    let activation = applied
        .iter()
        .find(|activation| activation.extension_id() == "com.example.runtime-skill")
        .expect("the package applied");
    let names: Vec<String> = agent
        .all_skills()
        .into_iter()
        .map(|skill| skill.metadata.name)
        .collect();
    assert!(
        names.iter().any(|name| name == "runtime-tour"),
        "a skill the runtime registers at activation reaches the agent: {names:?}"
    );

    let _ = agent.react("do the tour".to_string()).await;
    assert!(
        saw.load(Ordering::Relaxed),
        "the model's load_skill must open the runtime-registered skill"
    );

    // The activation owns the skill: suspending it takes the skill out, and
    // resuming must put it back, or the model keeps a stale catalogue entry it
    // cannot load.
    assert!(
        activation.suspend_live(&agent) > 0,
        "the activation suspends"
    );
    assert!(
        !agent
            .all_skills()
            .iter()
            .any(|skill| skill.metadata.name == "runtime-tour"),
        "a suspended activation's skill leaves the agent"
    );
    activation.resume_live(&agent);
    assert!(
        agent
            .all_skills()
            .iter()
            .any(|skill| skill.metadata.name == "runtime-tour"),
        "resuming the activation restores its runtime-registered skill"
    );
    host.shutdown().await;
}

/// The real Cordis example declares its skill as a file AND registers it from
/// `apply(ctx)`; either form must reach the agent, and the runtime's
/// materialized copy must not shadow it into an empty directory.
#[cfg_attr(
    windows,
    ignore = "cordis bridge cannot resolve a drive-letter entry on Windows"
)]
#[tokio::test]
async fn a_cordis_packages_skill_reaches_the_agent() {
    let root = scratch("cordis-skill");
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
        .expect("status");
    assert!(status.applied, "the package activates: {:?}", status.error);

    let mut agent = ::agent::Agent::from_config(::agent::AgentConfig::default());
    let mut registry = ::agent::plugins::PluginRegistry::default();
    let applied = host.apply_with_fibers(&mut agent, &mut registry, None, "session");
    let activation = applied
        .iter()
        .find(|activation| activation.extension_id() == "com.example.cordis-echo")
        .expect("the package applied");
    let names: Vec<String> = agent
        .all_skills()
        .into_iter()
        .map(|skill| skill.metadata.name)
        .collect();
    assert!(
        names.iter().any(|name| name == "greet"),
        "the cordis package's skill reaches the agent: {names:?}"
    );

    // The activation owns the skill: suspending it takes the skill out, and
    // resuming puts back the same content without re-reading the package.
    assert!(
        activation.suspend_live(&agent) > 0,
        "the activation suspends"
    );
    assert!(
        !agent
            .all_skills()
            .iter()
            .any(|skill| skill.metadata.name == "greet"),
        "a suspended activation's skill leaves the agent"
    );
    activation.resume_live(&agent);
    assert!(
        agent
            .all_skills()
            .iter()
            .any(|skill| skill.metadata.name == "greet"),
        "a resumed activation keeps its skill"
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

/// A package that contributes ONLY a skill must land in the agent's skill set:
/// the model's catalogue and its `load_skill` tool both read `Agent::all_skills`.
#[tokio::test]
async fn a_skill_only_package_reaches_the_agent_skill_set() {
    let root = scratch("skills-only");
    std::fs::create_dir_all(root.join("workspace")).expect("workspace");
    let source = root.join("source");
    std::fs::create_dir_all(source.join("skills/tour")).expect("skill dir");
    std::fs::write(
        source.join("extension.toml"),
        r#"
schema_version = "2.0"
[extension]
id = "com.example.tour"
name = "Tour"
version = "1.0.0"
description = "a skill-only package"
[compatibility]
host = ">=3.0.0, <4.0.0"
extension_api = "^1.0"
[runtime]
kind = "declarative"
[capabilities]
required = ["agent.skills.provide"]
[[contribution]]
kind = "skill"
id = "tour"
[contribution.payload]
"#,
    )
    .expect("manifest");
    std::fs::write(
        source.join("skills/tour/SKILL.md"),
        "---\nname: tour\ndescription: a package-driven tour\n---\n\nPACKAGE TOUR BODY\n",
    )
    .expect("skill");

    let store = ExtensionStore::open(&root).expect("store");
    let package = store.install(&source).expect("install");
    grant_all(
        &store,
        package.id(),
        &package.manifest.capabilities.required,
    );
    let mut host = ExtensionHost::open(config(&root)).expect("host");
    let status = host
        .statuses()
        .into_iter()
        .find(|status| status.id == "com.example.tour")
        .expect("status");
    assert!(status.applied, "the package activates: {:?}", status.error);
    assert!(!status.skills.is_empty(), "skills: {:?}", status.skills);

    let saw = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut provider = ::agent::agent::agentic::LlmProvider::new();
    provider.register_vendor(
        "mock".to_string(),
        Box::new(SkillLoader {
            skill: "tour",
            body: "PACKAGE TOUR BODY",
            calls: AtomicUsize::new(0),
            saw: std::sync::Arc::clone(&saw),
        }),
    );
    let mut agent = ::agent::Agent::new(
        ::agent::AgentConfig {
            model: "mock-model".to_string(),
            ..Default::default()
        },
        std::sync::Arc::new(provider),
    );
    let mut registry = ::agent::plugins::PluginRegistry::default();
    let applied = host.apply_with_fibers(&mut agent, &mut registry, None, "session");
    assert!(!applied.is_empty(), "the package applies to the agent");
    let catalogue: Vec<String> = agent
        .model_skills()
        .into_iter()
        .map(|skill| skill.name)
        .collect();
    assert!(
        catalogue.iter().any(|name| name == "tour"),
        "the package's skill reaches the model catalogue: {catalogue:?}"
    );
    assert!(
        agent
            .all_skills()
            .iter()
            .any(|skill| skill.metadata.name == "tour"),
        "and the loader can open it"
    );

    let _ = agent.react("do the tour".to_string()).await;
    assert!(
        saw.load(std::sync::atomic::Ordering::Relaxed),
        "the model's load_skill must return the package's instructions"
    );
    host.shutdown().await;
}

/// A model that asks for `load_skill` once, then reports whether the tool
/// result carried the requested package's instructions.
struct SkillLoader {
    skill: &'static str,
    body: &'static str,
    calls: AtomicUsize,
    saw: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait::async_trait]
impl agent::LlmClient<agent::agent::AgentSession, agent::agent::AgentReactContext> for SkillLoader {
    async fn complete(
        &self,
        _persona: Option<String>,
        _req: react::llm::LlmRequest,
        _session: &mut agent::agent::AgentSession,
        context: &mut agent::agent::AgentReactContext,
    ) -> react::llm::LlmResponseResult {
        if self.calls.fetch_add(1, Ordering::Relaxed) == 0 {
            return Ok(e2e_tool_call(
                "load_skill",
                &format!(r#"{{"name":"{}"}}"#, self.skill),
            ));
        }
        let seen = context
            .instructions
            .iter()
            .any(|instruction| instruction.instruction.contains(self.body));
        self.saw.store(seen, Ordering::Relaxed);
        Ok(e2e_text("done"))
    }

    async fn stream_complete(
        &self,
        _persona: Option<String>,
        _req: react::llm::LlmRequest,
        _session: &mut agent::agent::AgentSession,
        _context: &mut agent::agent::AgentReactContext,
    ) -> Result<react::llm::TokenStream, react::llm::LlmError> {
        Ok(Box::pin(futures::stream::iter(vec![Ok(
            react::llm::StreamToken::Done,
        )])))
    }

    fn provider_name(&self) -> &'static str {
        "skill-loader"
    }
}

fn e2e_tool_call(name: &str, arguments: &str) -> react::llm::LlmResponse {
    use react::llm::vendor::{ChatCompletionResponse, ChatMessage, Choice, FunctionCall, ToolCall};
    react::llm::LlmResponse::OpenAI(ChatCompletionResponse {
        id: "mock".to_string(),
        object: "chat.completion".to_string(),
        created: 0,
        model: "mock".to_string(),
        choices: vec![Choice {
            index: 0,
            message: ChatMessage {
                role: "assistant".to_string(),
                content: None,
                tool_calls: Some(vec![ToolCall {
                    id: "call-1".to_string(),
                    r#type: "function".to_string(),
                    function: FunctionCall {
                        name: Some(name.to_string()),
                        arguments: Some(arguments.to_string()),
                    },
                }]),
                function_call: None,
                reasoning_content: None,
                extra: serde_json::Value::Object(serde_json::Map::new()),
            },
            finish_reason: Some("tool_calls".to_string()),
            stop_reason: None,
            logprobs: None,
        }],
        usage: None,
        system_fingerprint: None,
        nvext: None,
    })
}

fn e2e_text(content: &str) -> react::llm::LlmResponse {
    use react::llm::vendor::{ChatCompletionResponse, ChatMessage, Choice};
    react::llm::LlmResponse::OpenAI(ChatCompletionResponse {
        id: "mock".to_string(),
        object: "chat.completion".to_string(),
        created: 0,
        model: "mock".to_string(),
        choices: vec![Choice {
            index: 0,
            message: ChatMessage {
                role: "assistant".to_string(),
                content: Some(content.to_string()),
                tool_calls: None,
                function_call: None,
                reasoning_content: None,
                extra: serde_json::Value::Object(serde_json::Map::new()),
            },
            finish_reason: Some("stop".to_string()),
            stop_reason: None,
            logprobs: None,
        }],
        usage: None,
        system_fingerprint: None,
        nvext: None,
    })
}

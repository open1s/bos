use std::sync::Arc;

use resource::explorer::{handler_for, Explorer, Row};
use resource::policy::{Effect, PolicyDoc, Rule, SharedPolicy};
use resource::prelude::*;
use resource::transport::inprocess::InProcessTransport;

fn allow_all() -> SharedPolicy {
    SharedPolicy::new(PolicyDoc {
        admins: vec!["admin".to_string()],
        rules: vec![Rule {
            agents: vec!["*".to_string()],
            uris: vec!["*".to_string()],
            actions: vec!["*".to_string()],
            effect: Effect::Allow,
        }],
        ..Default::default()
    })
}

async fn seeded_client() -> Arc<ResourceClient> {
    let mgr = Arc::new(ResourceManager::new(allow_all()));
    mgr.register(Box::new(MemResource::new("notes")), "agent1".to_string())
        .await
        .unwrap();
    // Remote node "n1" with its own memory store.
    let remote = Arc::new(ResourceManager::new(allow_all()));
    remote
        .register(Box::new(MemResource::new("n1/notes")), "agent1".to_string())
        .await
        .unwrap();
    let client = Arc::new(ResourceClient::new("agent1", Some(mgr), None));
    client.connect("n1", Arc::new(InProcessTransport::new(remote, "agent1")));
    client
}

#[tokio::test]
async fn describe_lists_local_plus_remote() {
    let client = seeded_client().await;
    let explorer = Explorer::new(client);
    let infos = explorer.describe(None).await.unwrap();
    let uris: Vec<&str> = infos.iter().map(|i| i.uri.as_str()).collect();
    assert!(uris.contains(&"mem://notes"), "local resource present: {uris:?}");
    assert!(uris.contains(&"mem://n1/notes"), "remote resource present: {uris:?}");
}

#[tokio::test]
async fn tree_walks_local_and_remote_identically() {
    let client = seeded_client().await;

    // Seed both stores with a key so listing has content.
    for uri in ["mem://notes", "mem://n1/notes"] {
        client
            .invoke(uri, ResourceAction::Put { key: "a".into(), value: b"1".to_vec() })
            .await
            .unwrap();
    }

    let explorer = Explorer::new(client);
    for uri in ["mem://notes", "mem://n1/notes"] {
        let rows = explorer.tree(uri, 2).await.unwrap();
        // First row: the resource itself; then its key as a leaf.
        match &rows[0] {
            Row::Resource { depth, info } => {
                assert_eq!(*depth, 0);
                assert_eq!(info.uri, uri);
            }
            _ => panic!("first row should be the resource"),
        }
        assert!(
            rows.iter().any(|r| matches!(r, Row::Child { depth: 1, name } if name == "a")),
            "key listed as child: {rows:?}"
        );
    }
}

#[tokio::test]
async fn tree_recurses_into_registered_children() {
    let mgr = Arc::new(ResourceManager::new(allow_all()));
    // A parent folder resource and a second folder resource for its `kv`
    // subdirectory registered under the child URI — the tree should recurse.
    let dir = std::env::temp_dir().join("rex_tree_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("kv")).unwrap();
    let parent = format!("folder://{}", dir.display());
    mgr.register(Box::new(FolderResource::new(&dir)), "agent1".to_string())
        .await
        .unwrap();
    mgr.register(Box::new(FolderResource::new(dir.join("kv"))), "agent1".to_string())
        .await
        .unwrap();
    let client = Arc::new(ResourceClient::new("agent1", Some(mgr), None));
    let explorer = Explorer::new(client);

    let rows = explorer.tree(&parent, 2).await.unwrap();
    let has_resource = rows
        .iter()
        .any(|r| matches!(r, Row::Resource { depth: 1, info } if info.uri.ends_with("/kv")));
    assert!(has_resource, "child uri recursed as resource: {rows:?}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn peek_reads_file_bytes() {
    let path = std::env::temp_dir().join("rex_peek_test");
    std::fs::write(&path, b"peek me").unwrap();
    let uri = format!("file://{}", path.display());

    let mgr = Arc::new(ResourceManager::new(allow_all()));
    mgr.register(Box::new(FileResource::new(&path)), "agent1".to_string())
        .await
        .unwrap();
    let client = Arc::new(ResourceClient::new("agent1", Some(mgr), None));
    // Open so Read has a handle, then peek.
    client.invoke(&uri, ResourceAction::Open).await.unwrap();
    let explorer = Explorer::new(client);
    let data = explorer.peek(&uri, 4).await.unwrap();
    assert_eq!(data, b"peek");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn handler_for_instantiates_by_scheme() {
    assert!(handler_for("mem://kv").is_some());
    assert!(handler_for("proc://shell").is_some());
    assert!(handler_for("sock://127.0.0.1:0").is_some());
    assert!(handler_for("folder:///tmp").is_some());
    assert!(handler_for("file:///tmp/x").is_some());
    assert!(handler_for("combine://node").is_some());
    assert!(handler_for("nope://x").is_none());
}

/// `Resource` handles and explorers interop: watch over a subscribed stream is
/// surfaced as an error (no event source) rather than a panic.
#[tokio::test]
async fn watch_on_non_event_resource_errors_cleanly() {
    let client = seeded_client().await;
    let explorer = Explorer::new(client);
    let res = explorer.watch("mem://notes", vec![]).await;
    assert!(res.is_err(), "mem has no event stream");
}

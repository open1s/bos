use resource::action::ResourceAction;
use resource::meta::ResourceStateLabel;
use resource::policy::{PolicyDoc, Rule, SharedPolicy, Effect};
use resource::prelude::*;
use resource::transport::inprocess::InProcessTransport;
use std::sync::Arc;

fn policy_doc() -> SharedPolicy {
    let doc = PolicyDoc {
        admins: vec!["admin".to_string()],
        rules: vec![Rule {
            agents: vec!["agent1".to_string()],
            uris: vec!["file://*".to_string()],
            actions: vec!["*".to_string()],
            effect: Effect::Allow,
        }],
        ..Default::default()
    };
    SharedPolicy::new(doc)
}

#[tokio::test]
async fn file_roundtrip_via_client() {
    let path = std::env::temp_dir().join("resource_test_file_roundtrip");
    let _ = std::fs::remove_file(&path);
    let uri = format!("file://{}", path.display());

    print!("Testing file roundtrip via ResourceClient: {}", uri);

    let mgr = Arc::new(ResourceManager::new(policy_doc()));
    mgr.register(Box::new(FileResource::new(&path)), "agent1".to_string())
        .await
        .unwrap();

    let client = ResourceClient::new("agent1", Some(mgr.clone()), None);

    assert!(matches!(client.invoke(&uri, ResourceAction::Open).await, Ok(ResourceOutput::Opened)));
    assert!(matches!(
        client
            .invoke(&uri, ResourceAction::Write { offset: 0, data: b"hello".to_vec() })
            .await,
        Ok(ResourceOutput::WriteOk { written: 5 })
    ));
    let out = client
        .invoke(&uri, ResourceAction::Read { offset: 0, len: 5 })
        .await
        .unwrap();
    match out {
        ResourceOutput::ReadOk { data } => assert_eq!(data, b"hello"),
        _ => panic!("expected ReadOk"),
    }
    assert!(matches!(
        client.invoke(&uri, ResourceAction::Close).await,
        Ok(ResourceOutput::Closed)
    ));

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn policy_denies_unauthorized_agent() {
    let path = std::env::temp_dir().join("resource_test_deny");
    let _ = std::fs::remove_file(&path);
    let uri = format!("file://{}", path.display());

    let mgr = Arc::new(ResourceManager::new(policy_doc()));
    mgr.register(Box::new(FileResource::new(&path)), "agent1".to_string())
        .await
        .unwrap();

    // "evil" is not in the allow rule -> deny-by-default.
    let client = ResourceClient::new("evil", Some(mgr), None);
    let res = client.invoke(&uri, ResourceAction::Open).await;
    assert!(matches!(res, Err(ResourceError::PolicyDenied { .. })));

    let _ = std::fs::remove_file(&path);
}

/// Discovery (`list`/`resolve`) is policy-gated too: an unauthorized agent must
/// not be able to enumerate or probe resources it may not use, across *all*
/// resource kinds.
#[tokio::test]
async fn discovery_is_policy_gated() {
    // Registry holds one resource of several kinds, but policy only allows
    // agent1 file access.
    let mgr = Arc::new(ResourceManager::new(policy_doc()));
    mgr.register(Box::new(FileResource::new("/tmp/gated_file")), "agent1".to_string())
        .await
        .unwrap();
    mgr.register(Box::new(MemResource::new("gated")), "agent1".to_string())
        .await
        .unwrap();
    mgr.register(Box::new(ProcResource::new("gated")), "agent1".to_string())
        .await
        .unwrap();

    // "evil" is denied everything.
    let evil = ResourceClient::new("evil", Some(mgr.clone()), None);

    // list: the denied agent sees nothing (not even mem/proc resources).
    let infos = evil.list(None).await.unwrap();
    assert!(infos.is_empty(), "denied agent must not see any resource, got {infos:?}");

    // resolve: the denied agent gets PolicyDenied, not the resource info.
    let res = evil.resolve("mem://gated").await;
    assert!(matches!(res, Err(ResourceError::PolicyDenied { .. })));

    // The allowed agent still sees only its permitted (file) resource.
    let agent1 = ResourceClient::new("agent1", Some(mgr), None);
    let infos = agent1.list(None).await.unwrap();
    let uris: Vec<&str> = infos.iter().map(|i| i.uri.as_str()).collect();
    assert!(uris.contains(&"file:///tmp/gated_file"));
    assert!(!uris.contains(&"mem://gated"), "file-only policy must hide mem resource");
    assert!(!uris.contains(&"proc://gated"), "file-only policy must hide proc resource");
}

#[tokio::test]
async fn discovery_lists_registered() {
    let path = std::env::temp_dir().join("resource_test_list");
    let _ = std::fs::remove_file(&path);
    let uri = format!("file://{}", path.display());

    let mgr = Arc::new(ResourceManager::new(policy_doc()));
    mgr.register(Box::new(FileResource::new(&path)), "agent1".to_string())
        .await
        .unwrap();
    let client = ResourceClient::new("agent1", Some(mgr), None);

    let infos = client.list(Some(ResourceType::Storage)).await.unwrap();
    assert!(infos.iter().any(|i| i.uri == uri));
    let resolved = client.resolve(&uri).await.unwrap();
    assert!(resolved.is_some());

    let _ = std::fs::remove_file(&path);
}

#[test]
fn envelope_roundtrips_over_rkyv() {
    let action = ResourceAction::Write {
        offset: 3,
        data: vec![1, 2, 3],
    };
    let bytes = resource::encode_action(&action).unwrap();
    let back = resource::decode_action(&bytes).unwrap();
    assert_eq!(action.name(), back.name());
}

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

#[tokio::test]
async fn folder_mkdir_list_remove() {
    let path = std::env::temp_dir().join("resource_test_folder");
    let _ = std::fs::remove_dir_all(&path);
    let uri = format!("folder://{}", path.display());

    let mgr = Arc::new(ResourceManager::new(allow_all()));
    mgr.register(Box::new(FolderResource::new(&path)), "agent1".to_string())
        .await
        .unwrap();
    let client = ResourceClient::new("agent1", Some(mgr), None);

    assert!(matches!(
        client
            .invoke(&uri, ResourceAction::MkDir { recursive: true })
            .await,
        Ok(ResourceOutput::MkDirOk)
    ));
    std::fs::write(path.join("a.txt"), b"a").unwrap();
    std::fs::write(path.join("b.txt"), b"b").unwrap();
    let out = client.invoke(&uri, ResourceAction::List { pattern: None }).await.unwrap();
    match out {
        ResourceOutput::Listed { entries } => assert_eq!(entries.len(), 2),
        _ => panic!("expected Listed"),
    }
    assert!(matches!(
        client
            .invoke(&uri, ResourceAction::Remove { recursive: true })
            .await,
        Ok(ResourceOutput::Removed)
    ));
}

#[tokio::test]
async fn proc_spawn_wait_and_kill() {
    let mgr = Arc::new(ResourceManager::new(allow_all()));

    // true -> exit 0
    mgr.register(Box::new(ProcResource::new("p1")), "agent1".to_string())
        .await
        .unwrap();
    let c1 = ResourceClient::new("agent1", Some(mgr.clone()), None);
    let _ = c1
        .invoke("proc://p1", ResourceAction::Spawn { args: vec!["true".into()], env: vec![] })
        .await
        .unwrap();
    let out = c1.invoke("proc://p1", ResourceAction::Wait).await.unwrap();
    match out {
        ResourceOutput::Exited { code } => assert_eq!(code, 0),
        _ => panic!("expected Exited"),
    }

    // sleep 2 -> kill
    mgr.register(Box::new(ProcResource::new("p2")), "agent1".to_string())
        .await
        .unwrap();
    let c2 = ResourceClient::new("agent1", Some(mgr), None);
    let _ = c2
        .invoke(
            "proc://p2",
            ResourceAction::Spawn {
                args: vec!["sleep".into(), "2".into()],
                env: vec![],
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        c2.invoke("proc://p2", ResourceAction::Kill { signal: 9 }).await,
        Ok(ResourceOutput::Killed)
    ));
}

#[tokio::test]
async fn combine_fans_out_and_aggregates() {
    let mgr = Arc::new(ResourceManager::new(allow_all()));

    // Two independent KV children under one combine node.
    let combine = Box::new(CombineResource::new("c1"));
    combine
        .add_child("mem://a".into(), Box::new(MemResource::new("a")))
        .await;
    combine
        .add_child("mem://b".into(), Box::new(MemResource::new("b")))
        .await;
    mgr.register(combine, "agent1".to_string()).await.unwrap();

    let client = ResourceClient::new("agent1", Some(mgr), None);

    // Put fans out to both children; Get reads back the replicated value.
    client
        .invoke(
            "combine://c1",
            ResourceAction::Put { key: "k".into(), value: b"v".to_vec() },
        )
        .await
        .unwrap();

    let out = client
        .invoke("combine://c1", ResourceAction::Get { key: "k".into() })
        .await
        .unwrap();
    assert_eq!(out, ResourceOutput::Got { value: Some(b"v".to_vec()) });

    // List is the union across children.
    let out = client
        .invoke("combine://c1", ResourceAction::List { pattern: None })
        .await
        .unwrap();
    assert_eq!(out, ResourceOutput::Listed { entries: vec!["k".to_string()] });

    // Status/Open/Close go through the combine node itself.
    assert!(matches!(
        client.invoke("combine://c1", ResourceAction::Status).await,
        Ok(ResourceOutput::Status { state: ResourceStateLabel::Closed })
    ));
    assert!(matches!(
        client.invoke("combine://c1", ResourceAction::Open).await,
        Ok(ResourceOutput::Opened)
    ));
}

#[tokio::test]
async fn combine_get_first_hit_wins() {
    let mgr = Arc::new(ResourceManager::new(allow_all()));

    // Prepare child "b" with a key before hand it to the combine node.
    let mut b = MemResource::new("b");
    b.handle(ResourceAction::Put { key: "hit".into(), value: b"42".to_vec() })
        .await
        .unwrap();

    let combine = Box::new(CombineResource::new("c2"));
    // Child a (empty) sorts first; child b holds the key.
    combine
        .add_child("mem://a".into(), Box::new(MemResource::new("a")))
        .await;
    combine.add_child("mem://b".into(), Box::new(b)).await;
    mgr.register(combine, "agent1".to_string()).await.unwrap();

    let client = ResourceClient::new("agent1", Some(mgr), None);

    // Child a answers the miss (Ok(None)); no error, so first-success returns
    // Got(None) from a — the first child in sorted URI order wins.
    let out = client
        .invoke("combine://c2", ResourceAction::Get { key: "hit".into() })
        .await
        .unwrap();
    assert_eq!(out, ResourceOutput::Got { value: None });
}

#[tokio::test]
async fn mem_put_get_list() {
    let uri = "mem://test";
    let mgr = Arc::new(ResourceManager::new(allow_all()));
    mgr.register(Box::new(MemResource::new("test")), "agent1".to_string())
        .await
        .unwrap();
    let client = ResourceClient::new("agent1", Some(mgr), None);

    assert!(matches!(
        client.invoke(uri, ResourceAction::Put { key: "a".into(), value: b"x".to_vec() }).await,
        Ok(ResourceOutput::Put)
    ));
    let out = client.invoke(uri, ResourceAction::Get { key: "a".into() }).await.unwrap();
    match out {
        ResourceOutput::Got { value: Some(v) } => assert_eq!(v, b"x"),
        _ => panic!("expected Got Some"),
    }
    let out = client.invoke(uri, ResourceAction::List { pattern: None }).await.unwrap();
    match out {
        ResourceOutput::Listed { entries } => assert!(entries.contains(&"a".to_string())),
        _ => panic!("expected Listed"),
    }
}

#[tokio::test]
async fn sock_connect_send_recv_echo() {
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 64];
        let n = tokio::io::AsyncReadExt::read(&mut s, &mut buf).await.unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut s, &buf[..n]).await.unwrap();
    });

    let uri = "sock://127.0.0.1:echo";
    let mgr = Arc::new(ResourceManager::new(allow_all()));
    mgr.register(Box::new(SockResource::new("127.0.0.1:echo")), "agent1".to_string())
        .await
        .unwrap();
    let client = ResourceClient::new("agent1", Some(mgr), None);

    assert!(matches!(
        client
            .invoke(uri, ResourceAction::Connect { addr: addr.to_string() })
            .await,
        Ok(ResourceOutput::Connected)
    ));
    assert!(matches!(
        client
            .invoke(uri, ResourceAction::Send { data: b"hi".to_vec() })
            .await,
        Ok(ResourceOutput::Sent { sent: 2 })
    ));
    let out = client.invoke(uri, ResourceAction::Recv { max: 10 }).await.unwrap();
    match out {
        ResourceOutput::RecvOk { data } => assert_eq!(data, b"hi"),
        _ => panic!("expected RecvOk"),
    }
}

#[tokio::test]
async fn register_records_owner() {
    let path = std::env::temp_dir().join("resource_test_owner");
    let _ = std::fs::remove_dir_all(&path);
    let uri = format!("folder://{}", path.display());

    let mgr = Arc::new(ResourceManager::new(allow_all()));
    mgr.register(Box::new(FolderResource::new(&path)), "agent9".to_string())
        .await
        .unwrap();
    let client = ResourceClient::new("agent9", Some(mgr), None);

    let info = client.resolve(&uri).await.unwrap().expect("resolved");
    assert_eq!(info.owner, "agent9");

    // Cleanup metadata path only.
    let _ = std::fs::remove_dir_all(&path);
}

#[tokio::test]
async fn tool_invokes_resource_via_json() {
    let uri = "mem://tooltest";
    let mgr = Arc::new(ResourceManager::new(allow_all()));
    mgr.register(Box::new(MemResource::new("tooltest")), "agent1".to_string())
        .await
        .unwrap();
    let client = Arc::new(ResourceClient::new("agent1", Some(mgr), None));
    let tool = ResourceTool::new(uri, client);

    let put = tool
        .call(serde_json::json!({ "Put": { "key": "k", "value": "AQID" } }))
        .await
        .unwrap();
    assert_eq!(put, serde_json::json!("Put"));

    let got = tool
        .call(serde_json::json!({ "Get": { "key": "k" } }))
        .await
        .unwrap();
    // Binary is base64-encoded in the JSON tool surface (`AQID` == [1,2,3]).
    assert_eq!(got, serde_json::json!({ "Got": { "value": "AQID" } }));
}

/// Network resources addressed by host are operated through the same `invoke`
/// call as local ones: the client routes `scheme://host/...` to the registered
/// per-host endpoint transparently.
#[tokio::test]
async fn network_resources_routed_by_host() {
    // Permissive policy for this routing test (we exercise routing, not policy).
    let allow = SharedPolicy::new(PolicyDoc {
        admins: vec!["admin".to_string()],
        rules: vec![Rule {
            agents: vec!["*".to_string()],
            uris: vec!["*".to_string()],
            actions: vec!["*".to_string()],
            effect: Effect::Allow,
        }],
        ..Default::default()
    });

    // Local node has its own mem store.
    let local = Arc::new(ResourceManager::new(allow.clone()));
    local
        .register(Box::new(MemResource::new("local")), "agent1".to_string())
        .await
        .unwrap();

    // Two remote nodes, each exposed via an in-process transport.
    let h1 = Arc::new(ResourceManager::new(allow.clone()));
    h1.register(Box::new(MemResource::new("h1")), "agent1".to_string())
        .await
        .unwrap();
    let h2 = Arc::new(ResourceManager::new(allow.clone()));
    h2.register(Box::new(MemResource::new("h2")), "agent1".to_string())
        .await
        .unwrap();

    let client = Arc::new(ResourceClient::new("agent1", Some(local), None));
    client.register_remote(
        "h1",
        Arc::new(InProcessTransport::new(h1.clone(), "agent1")) as Arc<dyn Transport>,
    );
    client.register_remote(
        "h2",
        Arc::new(InProcessTransport::new(h2.clone(), "agent1")) as Arc<dyn Transport>,
    );

    // Local resource served locally.
    client
        .invoke("mem://local", ResourceAction::Put { key: "k".into(), value: b"local!".to_vec() })
        .await
        .unwrap();

    // Host-addressed resources routed to the right remote node. `h1` and `h2`
    // are independent stores, despite the identical call shape.
    client
        .invoke("mem://h1", ResourceAction::Put { key: "k".into(), value: b"host1".to_vec() })
        .await
        .unwrap();
    client
        .invoke("mem://h2", ResourceAction::Put { key: "k".into(), value: b"host2".to_vec() })
        .await
        .unwrap();

    let from_h1 = client.invoke("mem://h1", ResourceAction::Get { key: "k".into() }).await.unwrap();
    let from_h2 = client.invoke("mem://h2", ResourceAction::Get { key: "k".into() }).await.unwrap();
    assert_eq!(from_h1, ResourceOutput::Got { value: Some(b"host1".to_vec()) });
    assert_eq!(from_h2, ResourceOutput::Got { value: Some(b"host2".to_vec()) });

    // A `Resource` handle pins the URI and is used identically for either kind.
    let res = Resource::new(client.clone(), "mem://h1");
    let v = res.invoke(ResourceAction::Get { key: "k".into() }).await.unwrap();
    assert_eq!(v, ResourceOutput::Got { value: Some(b"host1".to_vec()) });

    // Discovery aggregates from local + every remote endpoint.
    let all = client.list(None).await.unwrap();
    let uris: Vec<&str> = all.iter().map(|i| i.uri.as_str()).collect();
    assert!(uris.contains(&"mem://local"));
    assert!(uris.contains(&"mem://h1"));
    assert!(uris.contains(&"mem://h2"));
}

/// The seamless property: one `ResourceClient` facade registers local and remote
/// resources and then *operates them with identical code*. The helper below puts
/// and gets through a `Resource` handle; it is passed both a local URI and a
/// remote URI, and the body never knows or cares which it is.
#[tokio::test]
async fn operate_local_and_remote_identically_via_facade() {
    let allow = SharedPolicy::new(PolicyDoc {
        admins: vec!["admin".to_string()],
        rules: vec![Rule {
            agents: vec!["*".to_string()],
            uris: vec!["*".to_string()],
            actions: vec!["*".to_string()],
            effect: Effect::Allow,
        }],
        ..Default::default()
    });

    // One facade, one agent identity.
    let client = Arc::new(ResourceClient::new(
        "agent1",
        Some(Arc::new(ResourceManager::new(allow.clone()))),
        None,
    ));

    // Local: registered directly through the facade.
    client
        .register(Box::new(MemResource::new("local")), "agent1".to_string())
        .await
        .unwrap();

    // Remote: a separate manager exposed as another node on "node1".
    let remote = Arc::new(ResourceManager::new(allow.clone()));
    remote
        .register(Box::new(MemResource::new("node1/notes")), "agent1".to_string())
        .await
        .unwrap();
    client.connect(
        "node1",
        Arc::new(InProcessTransport::new(remote, "agent1")),
    );

    // The *identical* operation, applied to a local and a remote resource.
    async fn poke(client: &ResourceClient, uri: &str, marker: &[u8]) {
        let res = client
            .invoke(uri, ResourceAction::Put { key: "k".into(), value: marker.to_vec() })
            .await
            .unwrap();
        assert_eq!(res, ResourceOutput::Put);
        let got = client
            .invoke(uri, ResourceAction::Get { key: "k".into() })
            .await
            .unwrap();
        assert_eq!(got, ResourceOutput::Got { value: Some(marker.to_vec()) });
    }

    poke(&client, "mem://local", b"local!").await;
    poke(&client, "mem://node1/notes", b"remote!").await;

    // A `Resource` handle is equally location-oblivious.
    let handle = client.resource("mem://node1/notes");
    let got = handle.invoke(ResourceAction::Get { key: "k".into() }).await.unwrap();
    assert_eq!(got, ResourceOutput::Got { value: Some(b"remote!".to_vec()) });
}

// ---------------------------------------------------------------------------
// VirtualNodeResource: aggregates remote managers through transports
// ---------------------------------------------------------------------------

#[tokio::test]
async fn vnode_fans_out_writes_and_aggregates_list() {
    use resource::transport::inprocess::InProcessTransport;
    use resource::transport::Transport;

    // Two separate managers, each hosting its own `mem://` resource.
    let mgr_a = Arc::new(ResourceManager::new(allow_all()));
    mgr_a.register(Box::new(MemResource::new("a")), "agent1".into()).await.unwrap();

    let mgr_b = Arc::new(ResourceManager::new(allow_all()));
    mgr_b.register(Box::new(MemResource::new("b")), "agent1".into()).await.unwrap();

    let vnode = Box::new(VirtualNodeResource::new(
        "pool1",
        "agent1",
        vec![
            ("mem://a".into(), Arc::new(InProcessTransport::new(mgr_a.clone(), "agent1")) as Arc<dyn Transport>),
            ("mem://b".into(), Arc::new(InProcessTransport::new(mgr_b.clone(), "agent1")) as Arc<dyn Transport>),
        ],
    ));

    let mgr = Arc::new(ResourceManager::new(allow_all()));
    mgr.register(vnode, "agent1".into()).await.unwrap();
    let client = ResourceClient::new("agent1", Some(mgr), None);

    // Put fans out to both members; Get reads first-success.
    client
        .invoke(
            "vnode://pool1",
            ResourceAction::Put { key: "k".into(), value: b"v".to_vec() },
        )
        .await
        .unwrap();

    let out = client
        .invoke("vnode://pool1", ResourceAction::Get { key: "k".into() })
        .await
        .unwrap();
    assert_eq!(out, ResourceOutput::Got { value: Some(b"v".to_vec()) });

    // List is the union of each member's List entries (the keys), not the member URIs.
    let out = client
        .invoke("vnode://pool1", ResourceAction::List { pattern: None })
        .await
        .unwrap();
    match out {
        ResourceOutput::Listed { entries } => {
            assert!(entries.contains(&"k".to_string()), "expected key 'k' in: {entries:?}");
        }
        other => panic!("expected Listed, got {:?}", other),
    }
}

#[tokio::test]
async fn vnode_write_failure_surfaces_failed_members() {
    use resource::transport::inprocess::InProcessTransport;
    use resource::transport::Transport;

    // mgr_a is alive; mgr_b has no resource → Put on mem://b fails.
    let mgr_a = Arc::new(ResourceManager::new(allow_all()));
    mgr_a.register(Box::new(MemResource::new("a")), "agent1".into()).await.unwrap();

    let mgr_b = Arc::new(ResourceManager::new(allow_all()));

    let vnode = Box::new(VirtualNodeResource::new(
        "pool2",
        "agent1",
        vec![
            ("mem://a".into(), Arc::new(InProcessTransport::new(mgr_a, "agent1")) as Arc<dyn Transport>),
            ("mem://b".into(), Arc::new(InProcessTransport::new(mgr_b, "agent1")) as Arc<dyn Transport>),
        ],
    ));

    let mgr = Arc::new(ResourceManager::new(allow_all()));
    mgr.register(vnode, "agent1".into()).await.unwrap();
    let client = ResourceClient::new("agent1", Some(mgr), None);

    let err = client
        .invoke(
            "vnode://pool2",
            ResourceAction::Put { key: "k".into(), value: b"v".to_vec() },
        )
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("failed on members"), "expected failure info in: {msg}");
    assert!(msg.contains("mem://b"), "expected failed URI in: {msg}");
}

#[tokio::test]
async fn vnode_with_transport_convenience_constructor() {
    use resource::transport::inprocess::InProcessTransport;
    use resource::transport::Transport;

    let mgr = Arc::new(ResourceManager::new(allow_all()));
    mgr.register(Box::new(MemResource::new("x")), "agent1".into()).await.unwrap();

    let transport: Arc<dyn Transport> = Arc::new(InProcessTransport::new(mgr, "agent1"));
    let vnode = Box::new(VirtualNodeResource::with_transport(
        "pool3",
        "agent1",
        transport,
        vec!["mem://x".into()],
    ));

    let outer = Arc::new(ResourceManager::new(allow_all()));
    outer.register(vnode, "agent1".into()).await.unwrap();
    let client = ResourceClient::new("agent1", Some(outer), None);

    client
        .invoke("vnode://pool3", ResourceAction::Put { key: "a".into(), value: b"1".into() })
        .await
        .unwrap();
    let out = client
        .invoke("vnode://pool3", ResourceAction::Get { key: "a".into() })
        .await
        .unwrap();
    assert_eq!(out, ResourceOutput::Got { value: Some(b"1".to_vec()) });
}

#[tokio::test]
async fn vnode_member_accessors() {
    use resource::transport::inprocess::InProcessTransport;
    use resource::transport::Transport;

    let mgr_a = Arc::new(ResourceManager::new(allow_all()));
    mgr_a.register(Box::new(MemResource::new("a")), "agent1".into()).await.unwrap();

    let mgr_b = Arc::new(ResourceManager::new(allow_all()));
    mgr_b.register(Box::new(MemResource::new("b")), "agent1".into()).await.unwrap();

    let vnode = VirtualNodeResource::new(
        "pool4",
        "agent1",
        vec![
            ("mem://a".into(), Arc::new(InProcessTransport::new(mgr_a, "agent1")) as Arc<dyn Transport>),
        ],
    );

    // Initially one member.
    assert_eq!(vnode.member_uris().await, vec!["mem://a"]);

    // Add a second member.
    vnode.add_member(
        "mem://b".into(),
        Arc::new(InProcessTransport::new(mgr_b, "agent1")),
    ).await;
    let mut uris = vnode.member_uris().await;
    uris.sort();
    assert_eq!(uris, vec!["mem://a", "mem://b"]);

    // Remove the first member.
    assert!(vnode.remove_member("mem://a").await);
    assert_eq!(vnode.member_uris().await, vec!["mem://b"]);

    // Removing a nonexistent member returns false.
    assert!(!vnode.remove_member("mem://nope").await);
}

//! The persistent replay store is fail-closed when it cannot be loaded
//! (issue #349).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use capability::composition::{Binding, ResolvedCapability, ResolvedComposition};
use capability::{
    CapabilityCall, CapabilityDescriptor, CapabilityError, CapabilityId, CapabilityKind,
    CapabilityOutput, CapabilityProvider, FederationTransport, NodeId, ProviderHealth, ProviderId,
    ProviderKind, VersionSource, WorkspaceId, WorkspaceRuntime,
};
use federation::{FederationMessage, FederationNode, InMemoryNetwork, MemoryAudit, NodeIdentity};
use semver::Version;
use serde_json::json;

struct Echo {
    provider: ProviderId,
    descriptor: CapabilityDescriptor,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl CapabilityProvider for Echo {
    fn id(&self) -> &ProviderId {
        &self.provider
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Builtin
    }

    async fn describe(&self) -> Result<Vec<CapabilityDescriptor>, CapabilityError> {
        Ok(vec![self.descriptor.clone()])
    }

    async fn invoke(&self, call: CapabilityCall<'_>) -> Result<CapabilityOutput, CapabilityError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(CapabilityOutput::json(call.input))
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Available
    }
}

fn identity(seed: u8) -> NodeIdentity {
    NodeIdentity::from_seed([seed; 32])
}

fn capability(name: &str) -> CapabilityId {
    CapabilityId::new("builtin", name).expect("a valid id")
}

fn descriptor(name: &str) -> CapabilityDescriptor {
    CapabilityDescriptor::new(
        capability(name),
        CapabilityKind::Tool,
        Version::new(1, 0, 0),
        VersionSource::Declared,
    )
}

fn runtime(descriptor: CapabilityDescriptor, calls: Arc<AtomicUsize>) -> Arc<WorkspaceRuntime> {
    let provider: Arc<dyn CapabilityProvider> = Arc::new(Echo {
        provider: ProviderId::builtin(),
        descriptor: descriptor.clone(),
        calls,
    });
    let plan = ResolvedComposition {
        id: "compose:builder".into(),
        version: Version::new(1, 0, 0),
        workspace: WorkspaceId::new("ws:builder").expect("a valid workspace id"),
        capabilities: vec![ResolvedCapability {
            descriptor,
            binding: Binding::Local {
                provider: ProviderId::builtin(),
            },
            decision: None,
        }],
        skipped: Vec::new(),
        budgets: capability::composition::Budgets::default(),
    };
    Arc::new(WorkspaceRuntime::instantiate(plan, &[provider]).expect("a runtime"))
}

fn enroll(node: &FederationNode, caller: &NodeId) {
    let store = node.peers();
    let mut peers = store.write().expect("peers");
    let now = capability::unix_ms();
    peers.enroll(caller.clone(), now, None);
    peers
        .grant(caller, capability("echo"), now, None)
        .expect("granted");
}

fn invoke_frame(caller: &NodeIdentity, provider: &NodeId, message_id: &str) -> Vec<u8> {
    use capability::MessageId;

    let payload = json!({ "capability": "builtin/echo", "input": { "value": 1 } });
    let mut message =
        FederationMessage::new("invoke", serde_json::to_vec(&payload).expect("bytes"));
    message.id = MessageId::new(message_id);
    federation::Envelope::seal(&message, caller, provider)
        .expect("sealed")
        .encode()
        .expect("encoded")
}

async fn recv_reply(inbox: &mut tokio::sync::mpsc::Receiver<Vec<u8>>) -> federation::Envelope {
    let reply = tokio::time::timeout(Duration::from_secs(5), inbox.recv())
        .await
        .expect("a reply arrives")
        .expect("inbox open");
    federation::Envelope::decode(&reply).expect("reply envelope")
}

fn scratch(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "federation-replay-{label}-{}-{}",
        std::process::id(),
        capability::unix_ms()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

/// Issue #349: a store file that does not exist starts empty, and the first
/// admitted identity creates it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_replay_store_starts_empty_and_the_first_commit_creates_it() {
    let dir = scratch("missing");
    let replay_path = dir.join("replay.json");
    assert!(!replay_path.exists(), "the store starts absent");

    let network = Arc::new(InMemoryNetwork::new());
    let caller_identity = identity(141);
    let provider_identity = identity(142);
    let caller_id = caller_identity.node_id();
    let provider_id = provider_identity.node_id();
    let (caller_transport, mut caller_inbox) = network.join(caller_id.clone());
    let (provider_transport, provider_inbox) = network.join(provider_id.clone());
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = FederationNode::new(provider_identity, Arc::new(provider_transport))
        .with_runtime(runtime(descriptor("echo"), calls.clone()))
        .with_replay_persistence(replay_path.clone());
    let task = provider.start(provider_inbox);
    enroll(&provider, &caller_id);
    tokio::time::sleep(Duration::from_millis(20)).await;

    caller_transport
        .send(
            &provider_id,
            invoke_frame(&caller_identity, &provider_id, "m-missing"),
        )
        .await
        .expect("send");
    let reply = recv_reply(&mut caller_inbox).await;
    assert_eq!(
        reply.payload["ok"],
        json!(true),
        "the node serves: {:?}",
        reply.payload
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let text = std::fs::read_to_string(&replay_path).expect("the commit created the store");
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
    assert!(
        parsed["entries"]
            .as_array()
            .is_some_and(|entries| !entries.is_empty()),
        "{parsed}"
    );

    task.abort();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Issue #349: a corrupt store is not replaced by an empty one. Side-effectful
/// requests are refused with a typed error, the unreadable file is never
/// overwritten, an identity admitted before the corruption can no longer
/// execute, and the explicit reset is the only way back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_corrupt_replay_store_fails_closed_until_the_operator_resets_it() {
    let dir = scratch("corrupt");
    let replay_path = dir.join("replay.json");

    let caller_identity = identity(151);
    let provider_identity = identity(152);
    let caller_id = caller_identity.node_id();
    let provider_id = provider_identity.node_id();
    let calls = Arc::new(AtomicUsize::new(0));

    // First life: the envelope is admitted and its identity committed.
    let network = Arc::new(InMemoryNetwork::new());
    let (caller_transport, mut caller_inbox) = network.join(caller_id.clone());
    let (provider_transport, provider_inbox) = network.join(provider_id.clone());
    let provider = FederationNode::new(provider_identity.clone(), Arc::new(provider_transport))
        .with_runtime(runtime(descriptor("echo"), calls.clone()))
        .with_replay_persistence(replay_path.clone());
    let task = provider.start(provider_inbox);
    enroll(&provider, &caller_id);
    tokio::time::sleep(Duration::from_millis(20)).await;
    caller_transport
        .send(
            &provider_id,
            invoke_frame(&caller_identity, &provider_id, "m-admitted"),
        )
        .await
        .expect("send");
    let reply = recv_reply(&mut caller_inbox).await;
    assert_eq!(reply.payload["ok"], json!(true), "{:?}", reply.payload);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let admitted = std::fs::read(&replay_path).expect("the store was created");
    assert!(!admitted.is_empty());

    task.abort();
    drop(provider);
    network.leave(&provider_id);

    // Corrupt it. The bytes are kept exactly, so the assertions below can prove
    // the node never wrote over them.
    let corrupt = b"this is not the replay store".to_vec();
    std::fs::write(&replay_path, &corrupt).expect("corrupt the store");

    // Second life: the node refuses side-effectful work.
    let (provider_transport, provider_inbox) = network.join(provider_id.clone());
    let audit = Arc::new(MemoryAudit::new());
    let mut restarted = FederationNode::new(provider_identity, Arc::new(provider_transport))
        .with_runtime(runtime(descriptor("echo"), calls.clone()))
        .with_replay_persistence(replay_path.clone())
        .with_audit(audit.clone());
    let task = restarted.start(provider_inbox);
    enroll(&restarted, &caller_id);
    tokio::time::sleep(Duration::from_millis(20)).await;

    // The identity admitted before the corruption must not execute again: the
    // store that proves it was admitted cannot be read.
    caller_transport
        .send(
            &provider_id,
            invoke_frame(&caller_identity, &provider_id, "m-admitted"),
        )
        .await
        .expect("send");
    let reply = recv_reply(&mut caller_inbox).await;
    assert_eq!(reply.payload["ok"], json!(false), "{:?}", reply.payload);
    assert_eq!(
        reply.payload["error"]["kind"],
        json!("unavailable"),
        "{:?}",
        reply.payload
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the previously admitted envelope must not run again"
    );

    // A fresh side-effectful request is refused too, and refuses before it can
    // overwrite anything.
    caller_transport
        .send(
            &provider_id,
            invoke_frame(&caller_identity, &provider_id, "m-fresh-before-reset"),
        )
        .await
        .expect("send");
    let reply = recv_reply(&mut caller_inbox).await;
    assert_eq!(reply.payload["error"]["kind"], json!("unavailable"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        std::fs::read(&replay_path).expect("the unreadable store is still there"),
        corrupt,
        "an unavailable store is never overwritten"
    );
    assert!(
        audit
            .events()
            .iter()
            .filter(|event| event.outcome == "unavailable")
            .count()
            >= 2,
        "every refusal is audited: {:?}",
        audit.events()
    );

    // Explicit recovery: the corrupt bytes are quarantined, not deleted.
    let quarantined = restarted
        .reset_replay_store()
        .expect("the operator resets the unavailable store");
    assert_eq!(
        std::fs::read(&quarantined).expect("the quarantined bytes are preserved"),
        corrupt
    );
    assert!(
        !replay_path.exists(),
        "the store path is free for a fresh store"
    );
    let reset = audit
        .events()
        .into_iter()
        .find(|event| event.operation == "replay_store_reset")
        .expect("the reset is audited");
    assert_eq!(reset.outcome, "ok");
    assert!(
        reset
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("discarded"),
        "{reset:?}"
    );

    // The node serves again, and the first commit creates a fresh store.
    caller_transport
        .send(
            &provider_id,
            invoke_frame(&caller_identity, &provider_id, "m-after-reset"),
        )
        .await
        .expect("send");
    let reply = recv_reply(&mut caller_inbox).await;
    assert_eq!(reply.payload["ok"], json!(true), "{:?}", reply.payload);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(replay_path.exists(), "the fresh store was committed");

    // A healthy store is not resettable: that would discard live protection.
    assert_eq!(
        restarted.reset_replay_store(),
        Err(federation::service::ReplayResetError::Healthy)
    );

    task.abort();
    let _ = std::fs::remove_dir_all(&dir);
}

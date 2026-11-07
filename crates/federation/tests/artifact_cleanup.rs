//! Artifact cleanup crosses the signed envelope and reports per-item outcomes.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use capability::{CapabilityId, InvocationContext, InvocationId, NodeId, Principal, WorkspaceId};
use federation::{
    ArtifactCleanupRequest, ArtifactRef, ArtifactStore, FederationNode, InMemoryNetwork,
    NodeIdentity,
};

struct MockStore {
    removed: AtomicUsize,
    refuse: bool,
}

#[async_trait]
impl ArtifactStore for MockStore {
    async fn remove(&self, _requester: &NodeId, artifact: &ArtifactRef) -> Result<bool, String> {
        if self.refuse || artifact.uri.contains("protected") {
            return Err("refused by the store".to_string());
        }
        self.removed.fetch_add(1, Ordering::SeqCst);
        Ok(true)
    }
}

fn context() -> InvocationContext {
    InvocationContext::local(
        InvocationId::new("inv:cleanup"),
        Principal::Local {
            session: "s".to_string(),
            preset: "p".to_string(),
        },
        WorkspaceId::new("ws:cleanup").expect("workspace"),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cleanup_reports_removed_and_failed_artifacts() {
    let network = Arc::new(InMemoryNetwork::new());
    let caller_identity = NodeIdentity::from_seed([201u8; 32]);
    let provider_identity = NodeIdentity::from_seed([202u8; 32]);
    let caller_id = caller_identity.node_id();
    let provider_id = provider_identity.node_id();
    let (caller_transport, caller_inbox) = network.join(caller_id.clone());
    let (provider_transport, provider_inbox) = network.join(provider_id.clone());
    let store = Arc::new(MockStore {
        removed: AtomicUsize::new(0),
        refuse: false,
    });
    let provider = Arc::new(
        FederationNode::new(provider_identity, Arc::new(provider_transport))
            .with_artifact_store(store.clone()),
    );
    let caller = Arc::new(FederationNode::new(
        caller_identity,
        Arc::new(caller_transport),
    ));
    let provider_task = provider.start(provider_inbox);
    let caller_task = caller.start(caller_inbox);
    let now = capability::unix_ms();
    let grant: CapabilityId = "builtin/read_file".parse().expect("capability");
    caller
        .peers()
        .write()
        .expect("peers")
        .enroll(provider_id.clone(), now, None);
    caller
        .peers()
        .write()
        .expect("peers")
        .grant(&provider_id, grant.clone(), now, None)
        .expect("grant");
    provider
        .peers()
        .write()
        .expect("peers")
        .enroll(caller_id.clone(), now, None);
    tokio::time::sleep(Duration::from_millis(20)).await;

    let request = ArtifactCleanupRequest::new(
        "cleanup-1",
        vec![
            ArtifactRef::new("ok", "file:///tmp/ok").with_retention(1),
            ArtifactRef::new("protected", "file:///tmp/protected"),
        ],
    );
    let response = caller
        .cleanup_artifacts(&provider_id, &request, &context())
        .await
        .expect("cleanup reply");

    assert_eq!(response.removed, vec!["file:///tmp/ok".to_string()]);
    assert_eq!(response.failed.len(), 1);
    assert_eq!(response.failed[0].uri, "file:///tmp/protected");
    assert_eq!(store.removed.load(Ordering::SeqCst), 1);

    provider_task.abort();
    caller_task.abort();
}

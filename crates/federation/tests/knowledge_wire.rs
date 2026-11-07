//! Knowledge federation runs over the authenticated envelope, not a side channel.

use std::sync::Arc;

use capability::{
    CapabilityError, InvocationContext, InvocationId, NodeId, Principal, WorkspaceId,
};
use federation::{
    knowledge_hash, InMemoryNetwork, KnowledgeAuthorizer, KnowledgeClass, KnowledgeItem,
    KnowledgePublication, KnowledgePurpose, KnowledgeQuery, KnowledgeService, NodeIdentity,
    Provenance,
};
use serde_json::json;
use std::sync::RwLock;

struct Allow;

impl KnowledgeAuthorizer for Allow {
    fn may_query(
        &self,
        _peer: &NodeId,
        _class: KnowledgeClass,
        _purpose: KnowledgePurpose,
        _scope: Option<&WorkspaceId>,
    ) -> bool {
        true
    }

    fn may_publish(&self, _peer: &NodeId) -> bool {
        true
    }
}

struct Deny;

impl KnowledgeAuthorizer for Deny {
    fn may_query(
        &self,
        _peer: &NodeId,
        _class: KnowledgeClass,
        _purpose: KnowledgePurpose,
        _scope: Option<&WorkspaceId>,
    ) -> bool {
        false
    }

    fn may_publish(&self, _peer: &NodeId) -> bool {
        false
    }
}

fn item(origin: &NodeId, id: &str, payload: serde_json::Value) -> KnowledgeItem {
    KnowledgeItem {
        id: id.to_string(),
        class: KnowledgeClass::Public,
        provenance: Provenance {
            origin: origin.clone(),
            source: format!("memory/{id}"),
            issued_at_ms: capability::unix_ms(),
            expires_at_ms: None,
            content_hash: knowledge_hash(&payload),
        },
        payload,
        retention_ms: None,
    }
}

fn context() -> InvocationContext {
    InvocationContext::local(
        InvocationId::new("inv:knowledge"),
        Principal::Local {
            session: "s".to_string(),
            preset: "p".to_string(),
        },
        WorkspaceId::new("ws:knowledge").expect("a workspace"),
    )
}

async fn pair(
    authorizer: Arc<dyn KnowledgeAuthorizer>,
) -> (
    Arc<federation::FederationNode>,
    Arc<federation::FederationNode>,
    Arc<RwLock<KnowledgeService>>,
    NodeId,
    Vec<tokio::task::JoinHandle<()>>,
) {
    let network = Arc::new(InMemoryNetwork::new());
    let caller_identity = NodeIdentity::from_seed([71; 32]);
    let provider_identity = NodeIdentity::from_seed([72; 32]);
    let caller_id = caller_identity.node_id();
    let provider_id = provider_identity.node_id();
    let (caller_transport, caller_inbox) = network.join(caller_id.clone());
    let (provider_transport, provider_inbox) = network.join(provider_id.clone());
    let service = Arc::new(RwLock::new(KnowledgeService::new()));
    let caller = Arc::new(federation::FederationNode::new(
        caller_identity,
        Arc::new(caller_transport),
    ));
    let provider = Arc::new(
        federation::FederationNode::new(provider_identity, Arc::new(provider_transport))
            .with_knowledge(Arc::clone(&service), authorizer),
    );
    let tasks = vec![caller.start(caller_inbox), provider.start(provider_inbox)];
    let now = capability::unix_ms();
    caller
        .peers()
        .write()
        .expect("peers")
        .enroll(provider_id.clone(), now, None);
    provider
        .peers()
        .write()
        .expect("peers")
        .enroll(caller_id.clone(), now, None);
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    (caller, provider, service, provider_id, tasks)
}

#[tokio::test]
async fn a_query_returns_remote_knowledge_with_provenance() {
    let (caller, _provider, service, provider_id, tasks) = pair(Arc::new(Allow)).await;
    service
        .write()
        .expect("knowledge service")
        .publish(
            &provider_id,
            &KnowledgePublication::new(vec![item(
                &provider_id,
                "k-1",
                json!({ "fact": "governed" }),
            )]),
            capability::unix_ms(),
        )
        .expect("published");

    let response = caller
        .query_knowledge(
            &provider_id,
            &KnowledgeQuery::new("q", KnowledgePurpose::Answer),
            &context(),
        )
        .await
        .expect("the query is answered");

    assert_eq!(response.items.len(), 1);
    assert_eq!(response.items[0].id, "k-1");
    assert_eq!(
        response.origin,
        federation::KnowledgeOrigin::Remote { node: provider_id }
    );
    for task in tasks {
        task.abort();
    }
}

#[tokio::test]
async fn a_publication_is_authenticated_and_stored() {
    let (caller, _provider, service, _provider_id, tasks) = pair(Arc::new(Allow)).await;
    let caller_id = caller.node();
    let accepted = caller
        .publish_knowledge(
            &_provider_id,
            &KnowledgePublication::new(vec![item(&caller_id, "k-2", json!({ "fact": 2 }))]),
            &context(),
        )
        .await
        .expect("published");
    assert_eq!(accepted, 1);
    assert!(service
        .read()
        .expect("knowledge service")
        .get("k-2")
        .is_some());
    for task in tasks {
        task.abort();
    }
}

#[tokio::test]
async fn authorization_and_freshness_are_enforced_over_the_wire() {
    let (caller, _provider, _service, provider_id, tasks) = pair(Arc::new(Deny)).await;

    let denied = caller
        .query_knowledge(
            &provider_id,
            &KnowledgeQuery::new("q", KnowledgePurpose::Answer),
            &context(),
        )
        .await
        .expect_err("the authorizer denies");
    assert!(
        matches!(denied, CapabilityError::Denied { .. }),
        "{denied:?}"
    );

    let mut stale = KnowledgeQuery::new("q-stale", KnowledgePurpose::Answer);
    stale.deadline_ms = Some(1);
    let stale_error = caller
        .query_knowledge(&provider_id, &stale, &context())
        .await
        .expect_err("the query expired");
    assert!(
        matches!(stale_error, CapabilityError::Timeout { .. }),
        "{stale_error:?}"
    );

    for task in tasks {
        task.abort();
    }
}

#[tokio::test]
async fn a_revoked_item_disappears_from_the_next_query() {
    let (caller, _provider, service, provider_id, tasks) = pair(Arc::new(Allow)).await;
    service
        .write()
        .expect("knowledge service")
        .publish(
            &provider_id,
            &KnowledgePublication::new(vec![item(&provider_id, "k-3", json!({ "fact": 3 }))]),
            capability::unix_ms(),
        )
        .expect("published");
    service
        .write()
        .expect("knowledge service")
        .revoke(&provider_id, "k-3")
        .expect("revoked");

    let response = caller
        .query_knowledge(
            &provider_id,
            &KnowledgeQuery::new("q", KnowledgePurpose::Answer),
            &context(),
        )
        .await
        .expect("authorized");
    assert!(response.items.is_empty());
    for task in tasks {
        task.abort();
    }
}

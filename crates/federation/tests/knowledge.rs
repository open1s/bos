//! Knowledge is governed: provenance, class, purpose, freshness, revocation.

use capability::NodeId;
use federation::{
    knowledge_hash, KnowledgeAudit, KnowledgeClass, KnowledgeError, KnowledgeItem, KnowledgeOrigin,
    KnowledgePublication, KnowledgePurpose, KnowledgeQuery, KnowledgeService, Provenance,
};
use serde_json::json;

fn node(value: &str) -> NodeId {
    NodeId::new(value).expect("a node id")
}

fn item(id: &str, class: KnowledgeClass, payload: serde_json::Value) -> KnowledgeItem {
    KnowledgeItem {
        id: id.to_string(),
        class,
        workspace: match class {
            KnowledgeClass::Workspace => {
                Some(capability::WorkspaceId::new("ws:test").expect("a valid workspace id"))
            }
            _ => None,
        },
        provenance: Provenance {
            origin: node("node:source"),
            source: format!("memory/{id}"),
            issued_at_ms: 100,
            expires_at_ms: None,
            content_hash: knowledge_hash(&payload),
        },
        payload,
        retention_ms: None,
    }
}

fn authorize_all(
    _peer: &NodeId,
    _class: KnowledgeClass,
    _purpose: KnowledgePurpose,
    _scope: Option<&capability::WorkspaceId>,
) -> bool {
    true
}

#[test]
fn query_publication_and_response_round_trip() {
    let query = KnowledgeQuery::new("q-1", KnowledgePurpose::Answer);
    let text = serde_json::to_string(&query).expect("query json");
    assert_eq!(
        serde_json::from_str::<KnowledgeQuery>(&text).expect("query"),
        query
    );

    let publication = KnowledgePublication::new(vec![item(
        "k-1",
        KnowledgeClass::Public,
        json!({ "fact": "the sky is blue" }),
    )]);
    let text = serde_json::to_string(&publication).expect("publication json");
    let decoded: KnowledgePublication = serde_json::from_str(&text).expect("publication");
    assert_eq!(decoded, publication);
    assert_eq!(decoded.schema, 1);
}

#[test]
fn an_unauthorized_query_returns_a_typed_denial_and_is_audited() {
    let mut service = KnowledgeService::new();
    service
        .publish(
            &node("node:source"),
            &KnowledgePublication::new(vec![item(
                "k-1",
                KnowledgeClass::Public,
                json!({ "fact": 1 }),
            )]),
            200,
        )
        .expect("published");
    let query = KnowledgeQuery::new("q-denied", KnowledgePurpose::Compose);
    let mut audit: Vec<KnowledgeAudit> = Vec::new();

    let error = service
        .query(
            &node("node:source"),
            &node("node:caller"),
            &query,
            200,
            |_, _, _, _| false,
            &mut audit,
        )
        .expect_err("the grant is absent");

    assert!(
        matches!(error, KnowledgeError::Unauthorized { .. }),
        "{error:?}"
    );
    assert_eq!(audit.len(), 1);
    assert!(audit[0].denied);
}

#[test]
fn stale_items_and_an_expired_query_are_refused() {
    let mut service = KnowledgeService::new();
    let mut expired = item("k-old", KnowledgeClass::Public, json!({ "fact": "old" }));
    expired.provenance.expires_at_ms = Some(150);
    service
        .publish(
            &node("node:source"),
            &KnowledgePublication::new(vec![expired]),
            100,
        )
        .expect("published while fresh");
    service
        .publish(
            &node("node:source"),
            &KnowledgePublication::new(vec![item(
                "k-new",
                KnowledgeClass::Public,
                json!({ "fact": "new" }),
            )]),
            200,
        )
        .expect("published");

    let mut audit = Vec::new();
    let response = service
        .query(
            &node("node:source"),
            &node("node:caller"),
            &KnowledgeQuery::new("q-fresh", KnowledgePurpose::Answer),
            200,
            authorize_all,
            &mut audit,
        )
        .expect("authorized");
    assert_eq!(
        response
            .items
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        vec!["k-new"],
        "the expired item is not revived"
    );

    let mut late = KnowledgeQuery::new("q-late", KnowledgePurpose::Answer);
    late.deadline_ms = Some(150);
    let error = service
        .query(
            &node("node:source"),
            &node("node:caller"),
            &late,
            200,
            authorize_all,
            &mut audit,
        )
        .expect_err("the deadline passed");
    assert_eq!(error, KnowledgeError::Stale);
}

#[test]
fn a_revoked_item_is_gone_and_cannot_be_republished() {
    let mut service = KnowledgeService::new();
    let publisher = node("node:source");
    service
        .publish(
            &publisher,
            &KnowledgePublication::new(vec![item(
                "k-1",
                KnowledgeClass::Public,
                json!({ "fact": 1 }),
            )]),
            200,
        )
        .expect("published");

    let error = service
        .revoke(&node("node:other"), "k-1")
        .expect_err("not the publisher");
    assert!(matches!(error, KnowledgeError::RevokeDenied { .. }));
    assert!(service.revoke(&publisher, "k-1").expect("revoked"));
    assert!(service.is_revoked("k-1"));
    let mut audit = Vec::new();
    let response = service
        .query(
            &publisher,
            &node("node:caller"),
            &KnowledgeQuery::new("q", KnowledgePurpose::Answer),
            200,
            authorize_all,
            &mut audit,
        )
        .expect("authorized");
    assert!(response.items.is_empty());
}

#[test]
fn provenance_must_name_the_authenticated_publisher_and_hash_the_payload() {
    let mut service = KnowledgeService::new();
    let mut wrong_origin = item("k-origin", KnowledgeClass::Public, json!({ "fact": 1 }));
    wrong_origin.provenance.origin = node("node:another");
    let error = service
        .publish(
            &node("node:source"),
            &KnowledgePublication::new(vec![wrong_origin]),
            200,
        )
        .expect_err("origin mismatch");
    assert!(matches!(error, KnowledgeError::BadProvenance { .. }));

    let mut wrong_hash = item("k-hash", KnowledgeClass::Public, json!({ "fact": 1 }));
    wrong_hash.provenance.content_hash = "00".repeat(32);
    let error = service
        .publish(
            &node("node:source"),
            &KnowledgePublication::new(vec![wrong_hash]),
            200,
        )
        .expect_err("hash mismatch");
    assert!(matches!(error, KnowledgeError::BadProvenance { .. }));
}

#[test]
fn class_ceiling_and_minimisation_are_enforced() {
    let mut service = KnowledgeService::new();
    service
        .publish(
            &node("node:source"),
            &KnowledgePublication::new(vec![
                item(
                    "k-public",
                    KnowledgeClass::Public,
                    json!({ "topic": "alpha" }),
                ),
                item(
                    "k-restricted",
                    KnowledgeClass::Restricted,
                    json!({ "topic": "alpha", "secret": true }),
                ),
                item(
                    "k-public-2",
                    KnowledgeClass::Public,
                    json!({ "topic": "alpha" }),
                ),
                item(
                    "k-other",
                    KnowledgeClass::Public,
                    json!({ "topic": "beta" }),
                ),
            ]),
            200,
        )
        .expect("published");

    let mut query = KnowledgeQuery::new("q", KnowledgePurpose::Verify);
    query.max_items = 1;
    query.terms = vec!["alpha".to_string()];
    let mut audit = Vec::new();
    let response = service
        .query(
            &node("node:source"),
            &node("node:caller"),
            &query,
            200,
            authorize_all,
            &mut audit,
        )
        .expect("authorized");

    assert_eq!(response.items.len(), 1);
    assert_eq!(response.items[0].id, "k-public");
    assert!(response.truncated, "a second alpha item was cut");
    assert_eq!(
        response.origin,
        KnowledgeOrigin::Remote {
            node: node("node:source")
        },
        "remote knowledge is never reported as local"
    );
}

/// A scoped query sees its own workspace's items plus unscoped public items,
/// never another workspace's knowledge.
#[test]
fn workspace_knowledge_answers_only_its_own_scope() {
    let source = node("node:source");
    let mut service = KnowledgeService::new();
    let mut one = item("k-ws1", KnowledgeClass::Public, json!({ "fact": "one" }));
    one.workspace = Some(capability::WorkspaceId::new("ws:one").expect("a valid workspace id"));
    let mut two = item("k-ws2", KnowledgeClass::Public, json!({ "fact": "two" }));
    two.workspace = Some(capability::WorkspaceId::new("ws:two").expect("a valid workspace id"));
    let public = item("k-public", KnowledgeClass::Public, json!({ "fact": "all" }));
    service
        .publish(
            &source,
            &KnowledgePublication::new(vec![one, two, public]),
            200,
        )
        .expect("published");

    let mut scoped = KnowledgeQuery::new("q1", KnowledgePurpose::Answer);
    scoped.scope = Some(capability::WorkspaceId::new("ws:one").expect("a valid workspace id"));
    scoped.max_items = 10;
    let mut audit = Vec::new();
    let response = service
        .query(
            &source,
            &node("node:caller"),
            &scoped,
            200,
            authorize_all,
            &mut audit,
        )
        .expect("the query runs");
    let ids: Vec<&str> = response.items.iter().map(|item| item.id.as_str()).collect();
    assert!(ids.contains(&"k-ws1"), "{ids:?}");
    assert!(!ids.contains(&"k-ws2"), "another workspace leaked: {ids:?}");
    assert!(
        ids.contains(&"k-public"),
        "an unscoped public item still answers"
    );

    let mut unscoped = KnowledgeQuery::new("q2", KnowledgePurpose::Answer);
    unscoped.max_items = 10;
    let response = service
        .query(
            &source,
            &node("node:caller"),
            &unscoped,
            200,
            authorize_all,
            &mut audit,
        )
        .expect("the query runs");
    let ids: Vec<&str> = response.items.iter().map(|item| item.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["k-public"],
        "an unscoped query sees only unscoped items"
    );
}
#[test]
fn a_revoked_grant_denies_the_next_query() {
    let mut service = KnowledgeService::new();
    service
        .publish(
            &node("node:source"),
            &KnowledgePublication::new(vec![item(
                "k-1",
                KnowledgeClass::Workspace,
                json!({ "fact": 1 }),
            )]),
            200,
        )
        .expect("published");
    let query = KnowledgeQuery::new("q", KnowledgePurpose::Answer);
    let mut audit = Vec::new();
    let allowed = std::cell::Cell::new(true);

    let first = service.query(
        &node("node:source"),
        &node("node:caller"),
        &query,
        200,
        |peer, class, purpose, scope| {
            let _ = (peer, class, purpose, scope);
            allowed.get()
        },
        &mut audit,
    );
    assert!(first.is_ok());
    allowed.set(false);
    let error = service
        .query(
            &node("node:source"),
            &node("node:caller"),
            &query,
            200,
            |_, _, _, _| allowed.get(),
            &mut audit,
        )
        .expect_err("the grant was revoked");
    assert!(matches!(error, KnowledgeError::Unauthorized { .. }));
}

#[test]
fn knowledge_contracts_ride_the_authenticated_envelope() {
    let query = KnowledgeQuery::new("q-envelope", KnowledgePurpose::Answer);
    let message = query.to_message().expect("a message");
    assert_eq!(message.kind, "knowledge_query");
    let identity = federation::NodeIdentity::from_seed([91; 32]);
    let recipient = node("node:target");
    let envelope = federation::Envelope::seal(&message, &identity, &recipient).expect("sealed");
    let encoded = envelope.encode().expect("encoded");
    let decoded = federation::Envelope::decode(&encoded).expect("decoded");
    assert_eq!(decoded.sender, identity.node_id());
    assert_eq!(decoded.recipient, recipient);
    assert_eq!(
        KnowledgeQuery::from_envelope(&decoded).expect("query"),
        query
    );

    let publication = KnowledgePublication::new(vec![item(
        "k-envelope",
        KnowledgeClass::Public,
        json!({ "fact": true }),
    )]);
    let message = publication.to_message().expect("a publication message");
    assert_eq!(message.kind, "knowledge_publication");
    assert_eq!(
        KnowledgePublication::from_message(&message).expect("publication"),
        publication
    );
}

#[test]
fn a_revoked_id_cannot_be_republished() {
    let mut service = KnowledgeService::new();
    let publisher = node("node:source");
    let publication = KnowledgePublication::new(vec![item(
        "k-revoked",
        KnowledgeClass::Public,
        json!({ "fact": 1 }),
    )]);
    service
        .publish(&publisher, &publication, 200)
        .expect("published");
    service.revoke(&publisher, "k-revoked").expect("revoked");

    let error = service
        .publish(&publisher, &publication, 200)
        .expect_err("a revoked id is not revived");
    assert!(matches!(error, KnowledgeError::Revoked { .. }), "{error:?}");
    assert!(service.get("k-revoked").is_none());
}

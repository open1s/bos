//! Identity, envelope, and transport compose into one federation frame.
//!
//! These cases pin the wire contract: the same bytes a peer receives are the
//! bytes it verifies, and a transport cannot silently drop a message.

use std::sync::Arc;

use capability::transport::{unix_ms, FederationMessage, FederationTransport};
use capability::{CapabilityError, DeliveryState};
use federation::envelope::{EnvelopeError, PROTOCOL, SUPPORTED_SCHEMA_MAX, SUPPORTED_SCHEMA_MIN};
use federation::{Envelope, InMemoryNetwork, NodeIdentity};
use serde_json::json;

fn alice() -> NodeIdentity {
    NodeIdentity::from_seed([1u8; 32])
}

fn bob() -> NodeIdentity {
    NodeIdentity::from_seed([2u8; 32])
}

fn message() -> FederationMessage {
    FederationMessage::new(
        "invoke",
        br#"{"capability":"builtin/echo_add","input":{"a":1,"b":2}}"#.to_vec(),
    )
    .with_correlation("corr-1")
    .with_idempotency_key("idem-1")
}

#[test]
fn an_identity_has_a_stable_node_id() {
    assert_eq!(alice().node_id(), alice().node_id());
    assert_ne!(alice().node_id(), bob().node_id());
    assert!(federation::identity::is_valid_node_id(&alice().node_id()));
}

#[test]
fn a_signature_verifies_and_a_tampered_message_does_not() {
    let identity = alice();
    let signature = identity.sign(b"hello");
    assert!(federation::identity::verify(
        &identity.node_id(),
        b"hello",
        &signature
    ));
    assert!(!federation::identity::verify(
        &identity.node_id(),
        b"hello!",
        &signature
    ));
}

#[test]
fn a_sealed_envelope_round_trips_through_its_wire_bytes() {
    let envelope = Envelope::seal(&message(), &alice(), &bob().node_id()).expect("sealed");
    let bytes = envelope.encode().expect("encoded");
    let decoded = Envelope::decode(&bytes).expect("decoded");

    assert_eq!(decoded, envelope);
    assert_eq!(decoded.sender, alice().node_id());
    assert_eq!(decoded.recipient, bob().node_id());
    assert_eq!(decoded.correlation.as_deref(), Some("corr-1"));
    assert_eq!(decoded.idempotency_key.as_deref(), Some("idem-1"));
    assert_eq!(
        decoded.open(unix_ms()).expect("valid"),
        &json!({ "capability": "builtin/echo_add", "input": { "a": 1, "b": 2 } })
    );
}

#[test]
fn tampering_with_the_payload_invalidates_the_signature() {
    let mut envelope = Envelope::seal(&message(), &alice(), &bob().node_id()).expect("sealed");
    envelope.payload = json!({ "capability": "builtin/shell", "input": { "command": "rm -rf /" } });

    assert!(!envelope.verify());
    assert_eq!(envelope.open(unix_ms()), Err(EnvelopeError::Signature));
}

#[test]
fn an_unknown_protocol_is_refused_before_anything_else() {
    let mut envelope = Envelope::seal(&message(), &alice(), &bob().node_id()).expect("sealed");
    envelope.protocol = "other.fed/9".to_string();

    assert_eq!(
        envelope.open(unix_ms()),
        Err(EnvelopeError::UnknownProtocol {
            found: "other.fed/9".to_string(),
            expected: PROTOCOL.to_string(),
        })
    );
}

#[test]
fn an_unknown_schema_version_is_refused() {
    let mut envelope = Envelope::seal(&message(), &alice(), &bob().node_id()).expect("sealed");
    envelope.schema_version = 2;

    assert_eq!(
        envelope.open(unix_ms()),
        Err(EnvelopeError::UnsupportedSchema {
            found: 2,
            min: SUPPORTED_SCHEMA_MIN,
            max: SUPPORTED_SCHEMA_MAX,
        })
    );
}

#[test]
fn an_expired_envelope_is_refused() {
    let message = message().with_expiry_ms(unix_ms().saturating_sub(1));
    let envelope = Envelope::seal(&message, &alice(), &bob().node_id()).expect("sealed");

    assert_eq!(envelope.open(unix_ms()), Err(EnvelopeError::Expired));
}

#[tokio::test]
async fn a_frame_reaches_the_peer_and_reports_sent() {
    let network = Arc::new(InMemoryNetwork::new());
    let (sender, _sender_inbox) = network.join(alice().node_id());
    let (peer, mut peer_inbox) = network.join(bob().node_id());

    let session = sender.connect(peer.node()).await.expect("a session");
    assert_eq!(&session.peer, peer.node());

    let state = sender
        .send(peer.node(), b"one frame".to_vec())
        .await
        .expect("sent");
    assert_eq!(state, DeliveryState::Sent);
    assert_eq!(peer_inbox.recv().await.expect("delivered"), b"one frame");
}

#[tokio::test]
async fn an_unknown_peer_is_reported_as_not_sent() {
    let network = Arc::new(InMemoryNetwork::new());
    let (sender, _inbox) = network.join(alice().node_id());

    let error = sender
        .send(&bob().node_id(), b"lost".to_vec())
        .await
        .expect_err("bob is not on the network");

    assert!(
        matches!(
            error,
            CapabilityError::Transport {
                delivered: DeliveryState::NotSent,
                ..
            }
        ),
        "{error:?}"
    );
}

#[tokio::test]
async fn two_nodes_exchange_a_sealed_frame_over_the_transport() {
    let network = Arc::new(InMemoryNetwork::new());
    let (sender, _sender_inbox) = network.join(alice().node_id());
    let (peer, mut peer_inbox) = network.join(bob().node_id());

    let envelope = Envelope::seal(&message(), &alice(), &bob().node_id()).expect("sealed");
    let frame = envelope.encode().expect("encoded");
    sender
        .send(peer.node(), frame)
        .await
        .expect("the frame leaves alice");

    let received = peer_inbox.recv().await.expect("the frame arrives at bob");
    let decoded = Envelope::decode(&received).expect("decoded");
    let payload = decoded.open(unix_ms()).expect("verified and fresh");

    assert_eq!(decoded.sender, alice().node_id());
    assert_eq!(payload["capability"], "builtin/echo_add");
}

//! Wire-format conformance vectors.
//!
//! These pin the exact bytes and signature of one envelope so a change to the
//! canonical form, the JSON field order, or the signature encoding is caught
//! even when both ends of the suite still agree with each other.

use capability::transport::FederationMessage;
use federation::envelope::{accepts_protocol, accepts_schema, EnvelopeError, PROTOCOL};
use federation::{Envelope, NodeIdentity};

/// The pinned signature for the vector below.
const VECTOR_SIGNATURE: &str =
    "AKzyGin0k/xpdIwr+q75bN1fDM7yMIeKj2aU7YOic+P/WJS/lUSkINPjXAUGd2AE/mg/yNo+nMQpDZh/TxlhCg==";

fn vector() -> Envelope {
    let sender = NodeIdentity::from_seed([1u8; 32]);
    let recipient = NodeIdentity::from_seed([2u8; 32]).node_id();
    let mut message = FederationMessage::new(
        "invoke",
        br#"{"capability":"builtin/echo","input":{"value":7}}"#.to_vec(),
    )
    .with_correlation("corr-vector")
    .with_idempotency_key("idem-vector");
    // A fixed id keeps the signed bytes deterministic.
    message.id = capability::transport::MessageId::new("msg-vector");
    Envelope::seal_at(&message, &sender, &recipient, 1_700_000_000_000).expect("a sealed vector")
}

#[test]
fn the_wire_vector_is_pinned() {
    let envelope = vector();
    assert_eq!(envelope.protocol, PROTOCOL);
    assert_eq!(envelope.schema_version, 1);
    assert_eq!(
        envelope.sender,
        NodeIdentity::from_seed([1u8; 32]).node_id()
    );
    assert_eq!(
        envelope.recipient,
        NodeIdentity::from_seed([2u8; 32]).node_id()
    );
    assert_eq!(envelope.issued_at_ms, 1_700_000_000_000);
    if VECTOR_SIGNATURE == "TBD" {
        panic!("vector signature: {}", envelope.signature);
    }
    assert_eq!(
        envelope.signature, VECTOR_SIGNATURE,
        "the signed bytes changed; this is a wire-format break"
    );
    let payload = envelope.open(1_700_000_000_001).expect("the vector opens");
    assert_eq!(payload["capability"], "builtin/echo");
    assert_eq!(payload["input"]["value"], 7);
}

#[test]
fn a_tampered_vector_is_refused() {
    let mut envelope = vector();
    envelope.payload["input"]["value"] = serde_json::json!(8);
    assert!(matches!(
        envelope.open(1_700_000_000_001),
        Err(EnvelopeError::Signature)
    ));
}

#[test]
fn an_unknown_field_does_not_break_the_signature() {
    let envelope = vector();
    let mut value = serde_json::to_value(&envelope).expect("json");
    value["future"] = serde_json::json!({ "additive": true });
    let bytes = serde_json::to_vec(&value).expect("bytes");
    let decoded = Envelope::decode(&bytes).expect("decodes");
    assert!(decoded.open(1_700_000_000_001).is_ok());
}

#[test]
fn the_compatibility_window_is_named() {
    assert!(accepts_protocol("bsh.fed/1"));
    assert!(!accepts_protocol("bsh.fed/2"));
    assert!(accepts_schema(1));
    assert!(!accepts_schema(2));

    let mut envelope = vector();
    envelope.schema_version = 2;
    match envelope.open(1_700_000_000_001) {
        Err(EnvelopeError::UnsupportedSchema { found, min, max }) => {
            assert_eq!((found, min, max), (2, 1, 1));
        }
        other => panic!("unexpected: {other:?}"),
    }
}
